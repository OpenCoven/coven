//! The filesystem half of the scoped read contract: serving an authorized
//! request beneath a bound root.
//!
//! A root is the directory opened when the reader is built. A request is
//! served only while the root's path still names that directory, so a root
//! renamed away or replaced refuses every request. The walk opens one
//! component at a time from the root's handle and never follows a link, so no
//! spelling can leave the root. Each component's identity, checked on the
//! opened handle and not only on its name, must stay on the root's filesystem
//! and must not be a protected resource. Protection matches three ways: by
//! the identity each protected path named when the reader was built, so a
//! protected resource renamed away stays refused; by the identity it names
//! when each request starts, so a case or Unicode alias of its name on a
//! filesystem that equates them still matches; and by its path beneath the
//! root, so a protected file replaced by an atomic save, or created after the
//! reader was built, is still refused even when that happens mid-request.
//!
//! After the open, the walk is repeated from the root by name. Every directory
//! and the target must still be what the root's own names lead to; a
//! directory renamed out of the root after the walk entered it fails here,
//! before anything is read.
//!
//! A listing shows, and a search scans, only the entries a read would admit:
//! no link, special file, hard-linked file, other filesystem or protected
//! resource, and no name the service could not be asked for. A search reads
//! every file through the same checked read, so it can never disclose content
//! a read would refuse.
//!
//! Mount crossings are refused by device, and on Linux also by mount id, which
//! catches a bind mount from the same filesystem. Only Unix platforms have the
//! reviewed walk; elsewhere every request is refused.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::scope::{Coverage, Grants, Operation, RelativePath, Request, RootKind, SearchPosition};
use super::{ReadPolicy, Refusal};
use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};

/// A filesystem object's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Identity {
    device: u64,
    inode: u64,
}

/// What one request may not touch: the identities the protected paths named
/// when the reader was built, which follow a protected resource renamed to
/// another name; the identities they name when the request starts, which catch
/// an alias of an existing protected name; and their paths beneath the root,
/// which catch whatever a protected name comes to hold during the request,
/// including a new inode from an atomic save.
#[derive(Debug, Default)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Protected {
    identities: Vec<Identity>,
    paths: Vec<Vec<String>>,
}

impl Protected {
    #[cfg_attr(not(unix), allow(dead_code))]
    fn holds(&self, identity: Identity, path: &[String]) -> bool {
        self.identities.contains(&identity)
            || self
                .paths
                .iter()
                .any(|protected| protected.as_slice() == path)
    }
}

#[cfg(test)]
thread_local! {
    /// A test fault: the next directory listing on this thread fails once it
    /// has scanned at least this many name bytes, as a directory that errors
    /// part way through being read would. No permission change can make a
    /// directory that has already opened fail to list.
    pub(crate) static FAIL_LISTING_AFTER: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// A point in serving a request, reported to the reader's probe. Tests use it
/// to change the filesystem at an exact step; production passes a probe that
/// does nothing. A search reports the steps of its directory and each file it scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The first filesystem access: the root is about to be rechecked.
    Root,
    /// Directory component `n` was listed and is about to be opened.
    BeforeDirectory(usize),
    /// Directory component `n` was opened.
    Directory(usize),
    /// The target was listed and is about to be opened.
    BeforeOpen,
    /// The target was opened, and the recheck from the root is next.
    Opened,
    /// The checks passed: content or entries are read next.
    Read,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryKind {
    File,
    Directory,
}

impl EntryKind {
    fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

/// One directory entry a listing shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub kind: EntryKind,
}

/// One line a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    pub path: RelativePath,
    pub line: u64,
    /// The line, cut to the policy's bytes on a character boundary.
    pub text: String,
}

/// What a request produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Body {
    File {
        content: String,
    },
    Listing {
        entries: Vec<Entry>,
        /// Where the next page starts, when there is one.
        next: Option<String>,
    },
    Search {
        pattern: String,
        matches: Vec<Match>,
        /// Where the next page starts, when the page filled.
        next: Option<SearchPosition>,
        /// Entries left unscanned: files too large, not text, or refused by
        /// the read, and entries or directories beneath the searched one that
        /// could not be examined, opened or listed.
        skipped: u64,
        /// False when the page filled, a limit stopped the scan, or a
        /// directory beneath the searched one could not be walked, so files
        /// in it may have gone unscanned.
        complete: bool,
    },
}

/// A response, bounded and buffered whole. The service commits its digest
/// before any of it is disclosed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    pub root: RootKind,
    pub path: RelativePath,
    pub body: Body,
}

impl Response {
    /// The response as the model receives it.
    pub(crate) fn to_json(&self) -> Value {
        let (root, path) = (self.root.name(), self.path.text());
        match &self.body {
            Body::File { content } => json!({
                "operation": "read_file",
                "root": root,
                "path": path,
                "content": content,
            }),
            Body::Listing { entries, next } => json!({
                "operation": "list_directory",
                "root": root,
                "path": path,
                "entries": entries
                    .iter()
                    .map(|entry| json!({"name": entry.name, "kind": entry.kind.name()}))
                    .collect::<Vec<_>>(),
                "next": next,
            }),
            Body::Search {
                pattern,
                matches,
                next,
                skipped,
                complete,
            } => json!({
                "operation": "search",
                "root": root,
                "path": path,
                "pattern": pattern,
                "matches": matches
                    .iter()
                    .map(|found| json!({
                        "path": found.path.text(),
                        "line": found.line,
                        "text": found.text,
                    }))
                    .collect::<Vec<_>>(),
                "next": next
                    .as_ref()
                    .map(|next| json!({"path": next.path.text(), "line": next.line})),
                "skipped": skipped,
                "complete": complete,
            }),
        }
    }

    /// The lowercase hex SHA-256 of the response's JCS text.
    pub(crate) fn digest(&self) -> String {
        // Every number in a response is a line number or a count bounded by
        // the policy, so canonicalization cannot lose precision.
        sha256_hex(&canonicalize(&self.to_json()).expect("a response is I-JSON"))
    }
}

/// One attempt's reader: its grant, bound roots, protected resources and the
/// budget it has used.
pub(crate) struct ScopedReader {
    grants: Grants,
    policy: ReadPolicy,
    roots: Vec<platform::BoundRoot>,
    /// The protected paths as the daemon supplied them, resolved again for
    /// every request.
    protected: Vec<PathBuf>,
    /// What the protected paths named when the reader was built. They stay
    /// refused for the whole attempt, so renaming a protected resource to an
    /// unprotected name inside a granted scope does not release it.
    protected_at_build: Vec<Identity>,
    calls: u32,
    disclosed: u64,
    scanned: u64,
}

impl ScopedReader {
    /// Builds a reader for `scopes` over `roots`. A root that is, or is inside,
    /// a protected resource refuses the reader. The protected paths are kept
    /// as paths: each request resolves them afresh, so protection follows a
    /// protected name through replacement and covers one created later.
    pub(crate) fn new(
        scopes: &[Value],
        roots: &[(RootKind, &Path)],
        protected: &[&Path],
        policy: ReadPolicy,
    ) -> Result<Self, Refusal> {
        let grants = Grants::from_scopes(scopes, &policy)?;
        let identities = platform::protected_identities(protected)?;
        let mut bound = Vec::with_capacity(roots.len());
        for (kind, path) in roots {
            if bound
                .iter()
                .any(|root: &platform::BoundRoot| root.kind == *kind)
            {
                return Err(Refusal::GrantUnsupported);
            }
            bound.push(platform::bind(*kind, path, &identities)?);
        }
        Ok(Self {
            grants,
            policy,
            roots: bound,
            protected: protected.iter().map(|path| path.to_path_buf()).collect(),
            protected_at_build: identities,
            calls: 0,
            disclosed: 0,
            scanned: 0,
        })
    }

    /// Serves one request.
    pub(crate) fn serve(&mut self, request: &Value) -> Result<Response, Refusal> {
        self.serve_with(request, &mut |_| {})
    }

    /// [`Self::serve`], reporting each step to `probe`. Every call counts
    /// against the policy, refused ones included. The lexical checks all run
    /// before the first filesystem access.
    pub(crate) fn serve_with(
        &mut self,
        request: &Value,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Response, Refusal> {
        if self.calls >= self.policy.calls {
            return Err(Refusal::LimitCalls);
        }
        self.calls += 1;
        let request = Request::parse(request, &self.policy)?;
        let coverage = self.grants.authorize(&request)?;
        let root = self
            .roots
            .iter()
            .find(|root| root.kind == request.root)
            .ok_or(Refusal::RootUnbound)?;
        let mut protected = platform::protect(&self.protected, root)?;
        protected
            .identities
            .extend_from_slice(&self.protected_at_build);
        let disclosure = self.policy.attempt_bytes - self.disclosed;
        let scan = self.policy.scan_bytes - self.scanned;
        let body = match request.operation {
            Operation::ReadFile => {
                let content = platform::read_beneath(
                    root,
                    request.path.components(),
                    &protected,
                    self.policy.file_bytes,
                    ContentBudget::Disclosure(disclosure),
                    probe,
                )
                .map_err(|error| error.refusal)?;
                let content = String::from_utf8(content).map_err(|_| Refusal::ResourceNotText)?;
                self.disclosed += content.len() as u64;
                Body::File { content }
            }
            Operation::ListDirectory { after } => {
                if scan == 0 {
                    return Err(Refusal::LimitScan);
                }
                let listing = match platform::list_beneath(
                    root,
                    &request.path,
                    &protected,
                    &self.policy,
                    after.as_deref(),
                    scan,
                    probe,
                ) {
                    Ok(listing) => listing,
                    Err(error) => {
                        self.scanned += error.scanned;
                        return Err(error.refusal);
                    }
                };
                self.scanned += listing.scanned;
                Body::Listing {
                    entries: listing.entries,
                    next: listing.next,
                }
            }
            Operation::Search { pattern, after } => {
                debug_assert_eq!(coverage, Coverage::Subtree);
                if scan == 0 {
                    return Err(Refusal::LimitScan);
                }
                let found = match platform::search_beneath(
                    &platform::Scope {
                        root,
                        protected: &protected,
                        policy: &self.policy,
                    },
                    &request.path,
                    &pattern,
                    after.as_ref(),
                    scan,
                    disclosure,
                    probe,
                ) {
                    Ok(found) => found,
                    Err(error) => {
                        self.scanned += error.scanned;
                        return Err(error.refusal);
                    }
                };
                self.scanned += found.scanned;
                self.disclosed += found.disclosed;
                Body::Search {
                    complete: !found.stopped && !found.partial && found.next.is_none(),
                    pattern,
                    matches: found.matches,
                    next: found.next,
                    skipped: found.skipped,
                }
            }
        };
        Ok(Response {
            root: request.root,
            path: request.path,
            body,
        })
    }
}

/// A listing page and what scanning it cost.
#[cfg_attr(not(unix), allow(dead_code))]
struct Listing {
    entries: Vec<Entry>,
    next: Option<String>,
    scanned: u64,
}

/// A refused read or enumeration still carries the scan budget it consumed.
#[cfg_attr(not(unix), allow(dead_code))]
struct ScanError {
    refusal: Refusal,
    scanned: u64,
}

impl From<Refusal> for ScanError {
    fn from(refusal: Refusal) -> Self {
        Self {
            refusal,
            scanned: 0,
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(not(unix), allow(dead_code))]
enum ContentBudget {
    Disclosure(u64),
    Scan(u64),
}

impl ContentBudget {
    #[cfg_attr(not(unix), allow(dead_code))]
    fn remaining(self) -> u64 {
        match self {
            Self::Disclosure(remaining) | Self::Scan(remaining) => remaining,
        }
    }
}

/// A search page and what it cost.
#[derive(Default)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Found {
    matches: Vec<Match>,
    next: Option<SearchPosition>,
    skipped: u64,
    /// A limit stopped the scan.
    stopped: bool,
    /// Something beneath the searched directory could not be examined or
    /// walked, so the files it holds were not all scanned.
    partial: bool,
    scanned: u64,
    disclosed: u64,
}

/// `line` cut to at most `bytes` on a character boundary, without the
/// carriage return of a CRLF ending.
#[cfg_attr(not(unix), allow(dead_code))]
fn excerpt(line: &str, bytes: usize) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut end = line.len().min(bytes);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].to_owned()
}

#[cfg(unix)]
mod platform {
    use std::ffi::OsStr;
    use std::io::{self, Read};
    use std::path::{Component, Path, PathBuf};

    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::ambient_authority;
    use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};

    use super::{
        excerpt, ContentBudget, Entry, EntryKind, Found, Identity, Listing, Match, Protected,
        ReadPolicy, Refusal, RelativePath, RootKind, ScanError, SearchPosition, Step,
    };

    pub(super) struct BoundRoot {
        pub kind: RootKind,
        path: PathBuf,
        dir: Dir,
        identity: Identity,
        mount: Option<u64>,
    }

    /// What a search runs under.
    pub(super) struct Scope<'a> {
        pub root: &'a BoundRoot,
        pub protected: &'a Protected,
        pub policy: &'a ReadPolicy,
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

    /// Resolves the protected paths for one request beneath `root`: the
    /// identity of what each names now, and each one's path beneath the root,
    /// both through its canonical form and through its canonical parent. The
    /// second spelling holds a name that does not exist yet, or a link's own
    /// name, so a protected name stays refused whatever it comes to hold.
    pub(super) fn protect(paths: &[PathBuf], root: &BoundRoot) -> Result<Protected, Refusal> {
        let borrowed: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let identities = protected_identities(&borrowed)?;
        let mut beneath = Vec::new();
        for path in paths {
            for resolved in [std::fs::canonicalize(path).ok(), through_parent(path)]
                .into_iter()
                .flatten()
            {
                let Ok(relative) = resolved.strip_prefix(&root.path) else {
                    continue;
                };
                let components: Option<Vec<String>> = relative
                    .components()
                    .map(|component| match component {
                        Component::Normal(name) => name.to_str().map(str::to_owned),
                        _ => None,
                    })
                    .collect();
                // A name that is not UTF-8 can never be requested, and an empty
                // path is the root, which binding already checked.
                if let Some(components) = components.filter(|components| !components.is_empty()) {
                    if !beneath.contains(&components) {
                        beneath.push(components);
                    }
                }
            }
        }
        Ok(Protected {
            identities,
            paths: beneath,
        })
    }

    /// `path` with its nearest existing ancestor canonicalized and the
    /// remaining names kept as written, or `None` when it has no such form.
    fn through_parent(path: &Path) -> Option<PathBuf> {
        let mut names: Vec<&OsStr> = Vec::new();
        let mut current = path;
        loop {
            names.push(current.file_name()?);
            current = current.parent()?;
            if let Ok(mut resolved) = std::fs::canonicalize(current) {
                resolved.extend(names.iter().rev());
                return Some(resolved);
            }
        }
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

    /// Rechecks that the root's path still names the bound directory, then
    /// enters each of `parents` from it. Returns the last directory and the
    /// identity of each one entered.
    fn enter(
        root: &BoundRoot,
        parents: &[String],
        protected: &Protected,
        probe: &mut dyn FnMut(Step),
    ) -> Result<(Dir, Vec<Identity>), Refusal> {
        probe(Step::Root);
        let named = Dir::open_ambient_dir(&root.path, ambient_authority())
            .map_err(|_| Refusal::RootMoved)?;
        if identity(&named.dir_metadata().map_err(unavailable)?) != root.identity {
            return Err(Refusal::RootMoved);
        }
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
            admit(identity(&listed), &parents[..=index], root, protected)?;
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
        Ok((directory, held))
    }

    pub(super) fn read_beneath(
        root: &BoundRoot,
        components: &[String],
        protected: &Protected,
        file_bytes: u64,
        budget: ContentBudget,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Vec<u8>, ScanError> {
        let remaining = budget.remaining();
        let (name, parents) = components
            .split_last()
            .expect("a parsed path has a component");
        let (directory, held) = enter(root, parents, protected, probe)?;

        let listed = list(&directory, name)?;
        let file_type = listed.file_type();
        if file_type.is_symlink() {
            return Err(Refusal::ResourceSymlink.into());
        }
        if file_type.is_dir() {
            return Err(Refusal::ResourceDirectory.into());
        }
        if !file_type.is_file() {
            return Err(Refusal::ResourceSpecial.into());
        }
        admit(identity(&listed), components, root, protected)?;
        if listed.nlink() != 1 {
            return Err(Refusal::ResourceMultiplyLinked.into());
        }
        probe(Step::BeforeOpen);
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
            return Err(Refusal::ResourceMoved.into());
        }
        if opened.nlink() != 1 {
            return Err(Refusal::ResourceMultiplyLinked.into());
        }
        if mount_id(&file)? != root.mount {
            return Err(Refusal::ResourceMountCrossing.into());
        }
        probe(Step::Opened);
        recheck(root, parents, &held, name, identity(&opened))?;

        if opened.len() > file_bytes {
            return Err(Refusal::LimitFileTooLarge.into());
        }
        if opened.len() > remaining {
            return Err(Refusal::LimitAttemptDisclosure.into());
        }
        probe(Step::Read);
        // Detect growth with one sentinel byte when the scan budget permits it. Otherwise validate the held file length at the boundary.
        let limit = file_bytes.min(remaining);
        let read_limit = match budget {
            ContentBudget::Disclosure(_) => limit + 1,
            ContentBudget::Scan(_) => remaining.min(limit + 1),
        };
        let content = read_content(&mut file, opened.len().min(limit), read_limit)?;
        let scanned = content.len() as u64;
        if scanned > limit {
            return Err(ScanError {
                refusal: if limit == file_bytes {
                    Refusal::LimitFileTooLarge
                } else {
                    Refusal::LimitAttemptDisclosure
                },
                scanned,
            });
        }
        // A search cannot spend a byte beyond its scan budget to detect growth. At the boundary, recheck the held file's length rather than reading a sentinel byte.
        if matches!(budget, ContentBudget::Scan(_)) && scanned == remaining {
            let length = file
                .metadata()
                .map_err(|error| ScanError {
                    refusal: unavailable(error),
                    scanned,
                })?
                .len();
            if length != scanned {
                return Err(ScanError {
                    refusal: Refusal::LimitAttemptDisclosure,
                    scanned,
                });
            }
        }
        Ok(content)
    }

    fn read_content(
        reader: impl Read,
        capacity: u64,
        read_limit: u64,
    ) -> Result<Vec<u8>, ScanError> {
        let mut content = Vec::with_capacity(capacity as usize);
        reader
            .take(read_limit)
            .read_to_end(&mut content)
            .map_err(|error| ScanError {
                refusal: unavailable(error),
                scanned: content.len() as u64,
            })?;
        Ok(content)
    }

    /// Opens the directory `components` names, with the same checks as a
    /// file read, including the recheck from the root.
    fn open_directory_beneath(
        root: &BoundRoot,
        components: &[String],
        protected: &Protected,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Dir, Refusal> {
        let (name, parents) = components
            .split_last()
            .expect("a parsed path has a component");
        let (directory, held) = enter(root, parents, protected, probe)?;
        let listed = list(&directory, name)?;
        if listed.file_type().is_symlink() {
            return Err(Refusal::ResourceSymlink);
        }
        if !listed.is_dir() {
            return Err(Refusal::ResourceNotDirectory);
        }
        admit(identity(&listed), components, root, protected)?;
        probe(Step::BeforeOpen);
        let opened = directory
            .open_dir_nofollow(name)
            .map_err(|error| classify_open(&directory, name, &error))?;
        let metadata = opened.dir_metadata().map_err(unavailable)?;
        if identity(&metadata) != identity(&listed) {
            return Err(Refusal::ResourceMoved);
        }
        if mount_id(&opened)? != root.mount {
            return Err(Refusal::ResourceMountCrossing);
        }
        probe(Step::Opened);
        recheck(root, parents, &held, name, identity(&metadata))?;
        Ok(opened)
    }

    pub(super) fn list_beneath(
        root: &BoundRoot,
        path: &RelativePath,
        protected: &Protected,
        policy: &ReadPolicy,
        after: Option<&str>,
        scan: u64,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Listing, ScanError> {
        let directory = open_directory_beneath(root, path.components(), protected, probe)?;
        probe(Step::Read);
        let (names, scanned) = names(&directory, policy, scan)?;
        let start = after.map_or(0, |after| {
            names.partition_point(|name| name.as_str() <= after)
        });
        let mut entries: Vec<Entry> = Vec::new();
        let mut next = None;
        for name in &names[start..] {
            let Ok(child) = path.child(name, policy) else {
                continue;
            };
            let Some((kind, _)) = visible(&directory, name, child.components(), root, protected)
            else {
                continue;
            };
            if entries.len() == policy.page_entries {
                next = entries.last().map(|entry| entry.name.clone());
                break;
            }
            entries.push(Entry {
                name: name.clone(),
                kind,
            });
        }
        Ok(Listing {
            entries,
            next,
            scanned,
        })
    }

    pub(super) fn search_beneath(
        scope: &Scope<'_>,
        start: &RelativePath,
        pattern: &str,
        after: Option<&SearchPosition>,
        scan: u64,
        disclosure: u64,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Found, ScanError> {
        let directory =
            open_directory_beneath(scope.root, start.components(), scope.protected, probe)?;
        probe(Step::Read);
        let mut search = Search {
            scope,
            pattern,
            after,
            scan,
            disclosure,
            found: Found::default(),
            probe,
        };
        // A searched directory that cannot be listed refuses: an empty result
        // would claim there was nothing to find.
        if let Err(refusal) = search.visit(&directory, start) {
            return Err(ScanError {
                refusal,
                scanned: search.found.scanned,
            });
        }
        Ok(search.found)
    }

    /// One search's walk, in path order: each directory's entries sorted by
    /// name, a directory before what is beneath it.
    struct Search<'a> {
        scope: &'a Scope<'a>,
        pattern: &'a str,
        after: Option<&'a SearchPosition>,
        scan: u64,
        disclosure: u64,
        found: Found,
        probe: &'a mut dyn FnMut(Step),
    }

    impl Search<'_> {
        fn done(&self) -> bool {
            self.found.stopped || self.found.next.is_some()
        }

        /// Something beneath the search could not be examined or walked:
        /// count it, and report the search as incomplete.
        fn unexamined(&mut self) {
            self.found.skipped += 1;
            self.found.partial = true;
        }

        /// Searches `directory`, which is `path`. Fails only when the
        /// directory itself cannot be listed for a reason other than the scan
        /// limit; the names it did scan are charged either way.
        fn visit(&mut self, directory: &Dir, path: &RelativePath) -> Result<(), Refusal> {
            let remaining = self.scan - self.found.scanned;
            let names = match names(directory, self.scope.policy, remaining) {
                Ok((names, scanned)) => {
                    self.found.scanned += scanned;
                    names
                }
                Err(error) => {
                    self.found.scanned += error.scanned;
                    if error.refusal == Refusal::LimitScan {
                        self.found.stopped = true;
                        return Ok(());
                    }
                    return Err(error.refusal);
                }
            };
            for name in names {
                if self.done() {
                    return Ok(());
                }
                let Ok(child) = path.child(&name, self.scope.policy) else {
                    continue;
                };
                // Everything at or beneath `child` comes before the
                // continuation point, so it was on an earlier page.
                if self
                    .after
                    .is_some_and(|after| child < after.path && !after.path.starts_with(&child))
                {
                    continue;
                }
                match visible(
                    directory,
                    &name,
                    child.components(),
                    self.scope.root,
                    self.scope.protected,
                ) {
                    Some((EntryKind::Directory, listed))
                        if child.components().len() < self.scope.policy.depth =>
                    {
                        let walked = enter_child(directory, &name, listed, self.scope.root)
                            .is_some_and(|next| self.visit(&next, &child).is_ok());
                        if !walked {
                            self.unexamined();
                        }
                    }
                    Some((EntryKind::File, _)) => self.scan(&child),
                    // A directory at the depth limit.
                    Some(_) => {}
                    None if unexaminable(directory, &name) => self.unexamined(),
                    // Left out on purpose, as a listing leaves it out, or gone
                    // since the directory was listed.
                    None => {}
                }
            }
            Ok(())
        }

        fn scan(&mut self, path: &RelativePath) {
            let after_line = match self.after {
                Some(after) if after.path == *path => after.line,
                _ => 0,
            };
            // The file is read again from the root, with every check a read
            // makes, so the walk's handles never decide what is disclosed.
            let content = match read_beneath(
                self.scope.root,
                path.components(),
                self.scope.protected,
                self.scope.policy.file_bytes,
                ContentBudget::Scan(self.scan - self.found.scanned),
                self.probe,
            ) {
                Ok(content) => content,
                Err(error) => {
                    self.found.scanned += error.scanned;
                    self.found.stopped = self.found.scanned == self.scan
                        || error.refusal == Refusal::LimitAttemptDisclosure;
                    if error.refusal != Refusal::LimitAttemptDisclosure {
                        self.found.skipped += 1;
                    }
                    return;
                }
            };
            self.found.scanned += content.len() as u64;
            self.found.stopped = self.found.scanned == self.scan;
            let Ok(text) = String::from_utf8(content) else {
                self.found.skipped += 1;
                return;
            };
            for (index, line) in text.split('\n').enumerate() {
                let number = index as u64 + 1;
                if number <= after_line || !line.contains(self.pattern) {
                    continue;
                }
                if self.found.matches.len() == self.scope.policy.page_matches {
                    self.found.next = self.found.matches.last().map(|last| SearchPosition {
                        path: last.path.clone(),
                        line: last.line,
                    });
                    return;
                }
                let text = excerpt(line, self.scope.policy.match_bytes);
                if self.found.disclosed + text.len() as u64 > self.disclosure {
                    self.found.stopped = true;
                    return;
                }
                self.found.disclosed += text.len() as u64;
                self.found.matches.push(Match {
                    path: path.clone(),
                    line: number,
                    text,
                });
            }
        }
    }

    /// The names in `directory` a request could ever use, sorted by bytes,
    /// and the name bytes it took to scan them all.
    fn names(
        directory: &Dir,
        policy: &ReadPolicy,
        scan: u64,
    ) -> Result<(Vec<String>, u64), ScanError> {
        let mut names = Vec::new();
        let mut scanned = 0u64;
        for entry in directory.entries().map_err(unavailable)? {
            let name = entry
                .map_err(|error| ScanError {
                    refusal: unavailable(error),
                    scanned,
                })?
                .file_name();
            let bytes = name.len() as u64;
            if bytes > scan - scanned {
                return Err(ScanError {
                    refusal: Refusal::LimitScan,
                    scanned: scan,
                });
            }
            scanned += bytes;
            #[cfg(test)]
            if super::FAIL_LISTING_AFTER
                .with(|after| after.get().is_some_and(|after| scanned >= after))
            {
                super::FAIL_LISTING_AFTER.with(|after| after.set(None));
                return Err(ScanError {
                    refusal: Refusal::ResourceUnavailable,
                    scanned,
                });
            }
            if let Some(name) = name
                .to_str()
                .filter(|name| RelativePath::is_name(name, policy))
            {
                names.push(name.to_owned());
            }
        }
        names.sort_unstable();
        Ok((names, scanned))
    }

    /// What a listing or search may show of `name`, which is `path` beneath
    /// the root, with its identity, or `None` when a read of it would be
    /// refused.
    fn visible(
        directory: &Dir,
        name: &str,
        path: &[String],
        root: &BoundRoot,
        protected: &Protected,
    ) -> Option<(EntryKind, Identity)> {
        let metadata = directory.symlink_metadata(name).ok()?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() || admit(identity(&metadata), path, root, protected).is_err() {
            return None;
        }
        let kind = if file_type.is_dir() {
            EntryKind::Directory
        } else if file_type.is_file() && metadata.nlink() == 1 {
            EntryKind::File
        } else {
            return None;
        };
        let listed = identity(&metadata);
        entry_on_mount(directory, name, listed, root).then_some((kind, listed))
    }

    /// O_PATH observes metadata without requiring permission to read a file's contents. The no-follow handle must still name the entry just listed.
    #[cfg(target_os = "linux")]
    fn entry_on_mount(directory: &Dir, name: &str, listed: Identity, root: &BoundRoot) -> bool {
        use rustix::fs::{openat, Mode, OFlags};
        let Ok(handle) = openat(
            directory,
            name,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) else {
            return false;
        };
        let file = cap_std::fs::File::from_std(handle.into());
        file.metadata()
            .is_ok_and(|metadata| identity(&metadata) == listed)
            && mount_id(&file).is_ok_and(|mount| mount == root.mount)
    }

    #[cfg(not(target_os = "linux"))]
    fn entry_on_mount(_directory: &Dir, _name: &str, _listed: Identity, root: &BoundRoot) -> bool {
        root.mount.is_none()
    }

    /// Whether `name` was left out of a walk only because it could not be
    /// examined at all, as in a directory that can be read but not searched.
    /// An entry that is gone since the listing was not missed.
    fn unexaminable(directory: &Dir, name: &str) -> bool {
        directory
            .symlink_metadata(name)
            .is_err_and(|error| error.kind() != io::ErrorKind::NotFound)
    }

    /// Opens a subdirectory a search found, only if it is still what was
    /// listed and on the root's mount.
    fn enter_child(directory: &Dir, name: &str, listed: Identity, root: &BoundRoot) -> Option<Dir> {
        let child = directory.open_dir_nofollow(name).ok()?;
        let metadata = child.dir_metadata().ok()?;
        (identity(&metadata) == listed && mount_id(&child).ok()? == root.mount).then_some(child)
    }

    /// Walks from the root by name again. Every directory the read held, and
    /// the target, must still be what the root's own names lead to.
    fn recheck(
        root: &BoundRoot,
        parents: &[String],
        held: &[Identity],
        name: &str,
        target: Identity,
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
        if identity(&named) != target {
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

    /// Whether the component at `path` beneath the root, with `component`'s
    /// identity, may be walked through or read.
    fn admit(
        component: Identity,
        path: &[String],
        root: &BoundRoot,
        protected: &Protected,
    ) -> Result<(), Refusal> {
        if component.device != root.identity.device {
            return Err(Refusal::ResourceMountCrossing);
        }
        if protected.holds(component, path) {
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

    #[cfg(test)]
    mod tests {
        use super::{
            bind, identity, read_content, visible, EntryKind, Protected, Refusal, RootKind,
        };
        use std::os::unix::fs::PermissionsExt;

        #[test]
        fn a_partial_io_error_preserves_consumed_content_bytes() {
            struct FailsAfterPrefix(std::io::Cursor<&'static [u8]>);
            impl std::io::Read for FailsAfterPrefix {
                fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                    if self.0.position() == 3 {
                        return Err(std::io::Error::other("synthetic fixture read failure"));
                    }
                    std::io::Read::read(&mut self.0, buffer)
                }
            }
            let error = match read_content(FailsAfterPrefix(std::io::Cursor::new(b"abc")), 3, 8) {
                Ok(_) => panic!("the injected read must fail"),
                Err(error) => error,
            };
            assert_eq!(error.refusal, Refusal::ResourceUnavailable);
            assert_eq!(error.scanned, 3);
        }

        #[test]
        fn visible_entries_must_match_the_roots_mount_even_on_the_same_device() {
            let scratch = tempfile::tempdir().unwrap();
            std::fs::write(scratch.path().join("file"), "fixture").unwrap();
            std::fs::set_permissions(
                scratch.path().join("file"),
                std::fs::Permissions::from_mode(0o0),
            )
            .unwrap();
            std::fs::create_dir(scratch.path().join("folder")).unwrap();
            let mut root = bind(RootKind::Project, scratch.path(), &[]).unwrap();
            for name in ["file", "folder"] {
                assert_eq!(
                    identity(&root.dir.symlink_metadata(name).unwrap()).device,
                    root.identity.device
                );
            }
            assert!(matches!(
                visible(
                    &root.dir,
                    "file",
                    &["file".to_owned()],
                    &root,
                    &Protected::default()
                ),
                Some((EntryKind::File, _))
            ));
            assert!(matches!(
                visible(
                    &root.dir,
                    "folder",
                    &["folder".to_owned()],
                    &root,
                    &Protected::default()
                ),
                Some((EntryKind::Directory, _))
            ));
            #[cfg(target_os = "linux")]
            assert!(
                root.mount.is_some(),
                "the real statx mount ID must be available"
            );
            root.mount = Some(root.mount.map_or(1, |id| id.wrapping_add(1)));
            assert!(visible(
                &root.dir,
                "file",
                &["file".to_owned()],
                &root,
                &Protected::default()
            )
            .is_none());
            assert!(visible(
                &root.dir,
                "folder",
                &["folder".to_owned()],
                &root,
                &Protected::default()
            )
            .is_none());
        }
    }
}

#[cfg(not(unix))]
mod platform {
    use std::path::Path;

    use std::path::PathBuf;

    use super::{
        ContentBudget, Found, Identity, Listing, Protected, ReadPolicy, Refusal, RelativePath,
        RootKind,
    };
    use super::{ScanError, SearchPosition, Step};

    pub(super) struct BoundRoot {
        pub kind: RootKind,
    }

    pub(super) struct Scope<'a> {
        pub root: &'a BoundRoot,
        pub protected: &'a Protected,
        pub policy: &'a ReadPolicy,
    }

    pub(super) fn protect(_paths: &[PathBuf], _root: &BoundRoot) -> Result<Protected, Refusal> {
        Err(Refusal::PlatformUnsupported)
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
        _protected: &Protected,
        _file_bytes: u64,
        _budget: ContentBudget,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Vec<u8>, ScanError> {
        Err(Refusal::PlatformUnsupported.into())
    }

    pub(super) fn list_beneath(
        _root: &BoundRoot,
        _path: &RelativePath,
        _protected: &Protected,
        _policy: &ReadPolicy,
        _after: Option<&str>,
        _scan: u64,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Listing, ScanError> {
        Err(Refusal::PlatformUnsupported.into())
    }

    pub(super) fn search_beneath(
        _scope: &Scope<'_>,
        _start: &RelativePath,
        _pattern: &str,
        _after: Option<&SearchPosition>,
        _scan: u64,
        _disclosure: u64,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Found, ScanError> {
        Err(Refusal::PlatformUnsupported.into())
    }
}
