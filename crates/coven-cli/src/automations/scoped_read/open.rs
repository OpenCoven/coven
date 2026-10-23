//! The filesystem half of the scoped read contract: serving an authorized
//! request beneath a bound root.
//!
//! A root is the directory opened when the reader is built. A request is
//! served only while the root's path still names that directory, so a root
//! renamed away or replaced refuses every request. The walk opens one
//! component at a time from the root's handle and never follows a link, so no
//! spelling can leave the root. Each component's identity, checked on the
//! opened handle and not only on its name, must stay on the root's filesystem
//! and must not be a protected resource. A protected resource is held by
//! identity, so a case or Unicode alias of its name on a filesystem that
//! equates them still matches.
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

use std::path::Path;

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

/// A point in serving a request, reported to the reader's probe. Tests use it
/// to change the filesystem at an exact step; production passes a probe that
/// does nothing. A search reports the steps of its directory, then nothing for
/// the files it scans.
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
        /// Files left unscanned: too large, not text, or refused by the read.
        skipped: u64,
        /// False when the page filled or a limit stopped the scan.
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
    protected: Vec<Identity>,
    calls: u32,
    disclosed: u64,
    scanned: u64,
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
        let disclosure = self.policy.attempt_bytes - self.disclosed;
        let scan = self.policy.scan_bytes - self.scanned;
        let body = match request.operation {
            Operation::ReadFile => {
                let content = platform::read_beneath(
                    root,
                    request.path.components(),
                    &self.protected,
                    self.policy.file_bytes,
                    disclosure,
                    probe,
                )?;
                let content = String::from_utf8(content).map_err(|_| Refusal::ResourceNotText)?;
                self.disclosed += content.len() as u64;
                Body::File { content }
            }
            Operation::ListDirectory { after } => {
                if scan == 0 {
                    return Err(Refusal::LimitScan);
                }
                let listing = platform::list_beneath(
                    root,
                    request.path.components(),
                    &self.protected,
                    &self.policy,
                    after.as_deref(),
                    scan,
                    probe,
                )?;
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
                let found = platform::search_beneath(
                    &platform::Scope {
                        root,
                        protected: &self.protected,
                        policy: &self.policy,
                    },
                    &request.path,
                    &pattern,
                    after.as_ref(),
                    scan,
                    disclosure,
                    probe,
                )?;
                self.scanned += found.scanned;
                self.disclosed += found.disclosed;
                Body::Search {
                    complete: !found.stopped && found.next.is_none(),
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

/// A search page and what it cost.
#[derive(Default)]
#[cfg_attr(not(unix), allow(dead_code))]
struct Found {
    matches: Vec<Match>,
    next: Option<SearchPosition>,
    skipped: u64,
    /// A limit stopped the scan.
    stopped: bool,
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
    use std::io::{self, Read};
    use std::path::{Path, PathBuf};

    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::ambient_authority;
    use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};

    use super::super::scope::is_excluded_name;
    use super::{
        excerpt, Entry, EntryKind, Found, Identity, Listing, Match, ReadPolicy, Refusal,
        RelativePath, RootKind, SearchPosition, Step,
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
        pub protected: &'a [Identity],
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
        protected: &[Identity],
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
        Ok((directory, held))
    }

    pub(super) fn read_beneath(
        root: &BoundRoot,
        components: &[String],
        protected: &[Identity],
        file_bytes: u64,
        remaining: u64,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Vec<u8>, Refusal> {
        let (name, parents) = components
            .split_last()
            .expect("a parsed path has a component");
        let (directory, held) = enter(root, parents, protected, probe)?;

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
            return Err(Refusal::ResourceMoved);
        }
        if opened.nlink() != 1 {
            return Err(Refusal::ResourceMultiplyLinked);
        }
        if mount_id(&file)? != root.mount {
            return Err(Refusal::ResourceMountCrossing);
        }
        probe(Step::Opened);
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

    /// Opens the directory `components` names, with the same checks as a
    /// file read, including the recheck from the root.
    fn open_directory_beneath(
        root: &BoundRoot,
        components: &[String],
        protected: &[Identity],
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
        admit(identity(&listed), root, protected)?;
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
        components: &[String],
        protected: &[Identity],
        policy: &ReadPolicy,
        after: Option<&str>,
        scan: u64,
        probe: &mut dyn FnMut(Step),
    ) -> Result<Listing, Refusal> {
        let directory = open_directory_beneath(root, components, protected, probe)?;
        probe(Step::Read);
        let (names, scanned) = names(&directory, policy, scan)?;
        let start = after.map_or(0, |after| {
            names.partition_point(|name| name.as_str() <= after)
        });
        let mut entries: Vec<Entry> = Vec::new();
        let mut next = None;
        for name in &names[start..] {
            let Some((kind, _)) = visible(&directory, name, root, protected) else {
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
    ) -> Result<Found, Refusal> {
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
        };
        search.visit(&directory, start);
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
    }

    impl Search<'_> {
        fn done(&self) -> bool {
            self.found.stopped || self.found.next.is_some()
        }

        fn visit(&mut self, directory: &Dir, path: &RelativePath) {
            let remaining = self.scan - self.found.scanned;
            let names = match names(directory, self.scope.policy, remaining) {
                Ok((names, scanned)) => {
                    self.found.scanned += scanned;
                    names
                }
                Err(Refusal::LimitScan) => {
                    self.found.stopped = true;
                    return;
                }
                Err(_) => return,
            };
            for name in names {
                if self.done() {
                    return;
                }
                let child = path.child(&name);
                // Everything at or beneath `child` comes before the
                // continuation point, so it was on an earlier page.
                if self
                    .after
                    .is_some_and(|after| child < after.path && !after.path.starts_with(&child))
                {
                    continue;
                }
                match visible(directory, &name, self.scope.root, self.scope.protected) {
                    Some((EntryKind::Directory, listed))
                        if child.components().len() < self.scope.policy.depth =>
                    {
                        if let Some(next) = enter_child(directory, &name, listed, self.scope.root) {
                            self.visit(&next, &child);
                        }
                    }
                    Some((EntryKind::File, _)) => self.scan(&child),
                    _ => {}
                }
            }
        }

        fn scan(&mut self, path: &RelativePath) {
            let first_line = match self.after {
                Some(after) if after.path == *path => after.line + 1,
                _ => 1,
            };
            // The file is read again from the root, with every check a read
            // makes, so the walk's handles never decide what is disclosed.
            let content = match read_beneath(
                self.scope.root,
                path.components(),
                self.scope.protected,
                self.scope.policy.file_bytes,
                self.scan - self.found.scanned,
                &mut |_| {},
            ) {
                Ok(content) => content,
                // The scan budget, passed as the read's remaining total.
                Err(Refusal::LimitAttemptDisclosure) => {
                    self.found.stopped = true;
                    return;
                }
                Err(_) => {
                    self.found.skipped += 1;
                    return;
                }
            };
            self.found.scanned += content.len() as u64;
            let Ok(text) = String::from_utf8(content) else {
                self.found.skipped += 1;
                return;
            };
            for (index, line) in text.split('\n').enumerate() {
                let number = index as u64 + 1;
                if number < first_line || !line.contains(self.pattern) {
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

    /// The names in `directory` a request could ever use, less the excluded
    /// ones, sorted by bytes, and the name bytes it took to scan them all.
    fn names(
        directory: &Dir,
        policy: &ReadPolicy,
        scan: u64,
    ) -> Result<(Vec<String>, u64), Refusal> {
        let mut names = Vec::new();
        let mut scanned = 0u64;
        for entry in directory.entries().map_err(unavailable)? {
            let name = entry.map_err(unavailable)?.file_name();
            scanned += name.len() as u64;
            if scanned > scan {
                return Err(Refusal::LimitScan);
            }
            if let Some(name) = name
                .to_str()
                .filter(|name| RelativePath::is_name(name, policy) && !is_excluded_name(name))
            {
                names.push(name.to_owned());
            }
        }
        names.sort_unstable();
        Ok((names, scanned))
    }

    /// What a listing or search may show of `name`, with its identity, or
    /// `None` when a read of it would be refused.
    fn visible(
        directory: &Dir,
        name: &str,
        root: &BoundRoot,
        protected: &[Identity],
    ) -> Option<(EntryKind, Identity)> {
        let metadata = directory.symlink_metadata(name).ok()?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() || admit(identity(&metadata), root, protected).is_err() {
            return None;
        }
        if file_type.is_dir() {
            Some((EntryKind::Directory, identity(&metadata)))
        } else if file_type.is_file() && metadata.nlink() == 1 {
            Some((EntryKind::File, identity(&metadata)))
        } else {
            None
        }
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

    use super::{Found, Identity, Listing, ReadPolicy, Refusal, RelativePath, RootKind};
    use super::{SearchPosition, Step};

    pub(super) struct BoundRoot {
        pub kind: RootKind,
    }

    pub(super) struct Scope<'a> {
        pub root: &'a BoundRoot,
        pub protected: &'a [Identity],
        pub policy: &'a ReadPolicy,
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

    pub(super) fn list_beneath(
        _root: &BoundRoot,
        _components: &[String],
        _protected: &[Identity],
        _policy: &ReadPolicy,
        _after: Option<&str>,
        _scan: u64,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Listing, Refusal> {
        Err(Refusal::PlatformUnsupported)
    }

    pub(super) fn search_beneath(
        _scope: &Scope<'_>,
        _start: &RelativePath,
        _pattern: &str,
        _after: Option<&SearchPosition>,
        _scan: u64,
        _disclosure: u64,
        _probe: &mut dyn FnMut(Step),
    ) -> Result<Found, Refusal> {
        Err(Refusal::PlatformUnsupported)
    }
}
