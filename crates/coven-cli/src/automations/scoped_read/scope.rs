//! The lexical half of the scoped read contract: which spellings a grant or a
//! request may use, and whether a request is authorized. Nothing here touches
//! the filesystem, so a refusal from this module happens before any open.
//!
//! Grant and request paths go through the same [`RelativePath::parse`]. The
//! authenticated scope objects are read, never rewritten: the enforcement form
//! is derived beside them, and their digests stay what was signed.

use serde_json::Value;

use super::{ReadPolicy, Refusal};

/// The fields of a profile filesystem scope, which is a closed object.
const FILESYSTEM_SCOPE_FIELDS: &[&str] = &["kind", "root", "path", "access", "recursive"];
/// Scope kinds that grant no filesystem access. The read service ignores
/// them; whether the envelope accepts them is the adapter's decision.
const NON_FILESYSTEM_SCOPE_KINDS: &[&str] = &["network", "credential", "evidence"];
/// The fields every request has.
const REQUEST_FIELDS: &[&str] = &["operation", "root", "path"];

/// Which bound directory a path is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RootKind {
    Project,
    Workspace,
}

impl RootKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "project" => Some(Self::Project),
            "workspace" => Some(Self::Workspace),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Workspace => "workspace",
        }
    }
}

/// A relative path in the one spelling the service interprets: non-empty
/// components joined by `/`, none of them `.` or `..`. Paths order component
/// by component, which is the order a walk visits them in when it takes each
/// directory's entries sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RelativePath {
    components: Vec<String>,
}

impl RelativePath {
    /// Checks run in a fixed order, so each spelling has one refusal: length,
    /// emptiness, an absolute or drive form, forbidden characters, depth, then
    /// each component in turn.
    pub(crate) fn parse(text: &str, policy: &ReadPolicy) -> Result<Self, Refusal> {
        if text.len() > policy.path_bytes {
            return Err(Refusal::PathLimit);
        }
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return Err(Refusal::PathEncodingUnsupported);
        }
        let drive = bytes[0].is_ascii_alphabetic() && bytes.get(1) == Some(&b':');
        if bytes[0] == b'/' || drive {
            return Err(Refusal::PathAbsolute);
        }
        // A backslash is a separator on Windows and a filename byte elsewhere,
        // and a colon names a drive or an alternate data stream: refuse both
        // rather than read them differently per platform.
        if text
            .chars()
            .any(|character| character == '\\' || character == ':' || character.is_control())
        {
            return Err(Refusal::PathEncodingUnsupported);
        }
        let components: Vec<&str> = text.split('/').collect();
        if components.len() > policy.depth {
            return Err(Refusal::PathLimit);
        }
        for component in &components {
            if *component == ".." {
                return Err(Refusal::PathTraversal);
            }
            if component.is_empty()
                || *component == "."
                // Windows drops a trailing dot or space, so `a.` names `a`.
                || component.ends_with('.')
                || component.ends_with(' ')
                || windows_device_name(component)
            {
                return Err(Refusal::PathEncodingUnsupported);
            }
        }
        Ok(Self {
            components: components.into_iter().map(str::to_owned).collect(),
        })
    }

    /// Whether `name` is one component the service would accept in a path.
    /// A directory entry with any other name can never be requested, so it is
    /// never listed or searched either.
    pub(crate) fn is_name(name: &str, policy: &ReadPolicy) -> bool {
        Self::parse(name, policy).is_ok_and(|path| path.components.len() == 1)
    }

    pub(crate) fn components(&self) -> &[String] {
        &self.components
    }

    /// This path with a name appended, checked against the same complete-path limits as a direct request.
    pub(crate) fn child(&self, name: &str, policy: &ReadPolicy) -> Result<Self, Refusal> {
        Self::parse(&format!("{}/{}", self.text(), name), policy)
    }

    pub(crate) fn text(&self) -> String {
        self.components.join("/")
    }

    pub(crate) fn starts_with(&self, prefix: &Self) -> bool {
        self.components.starts_with(&prefix.components)
    }
}

/// `CON`, `NUL`, `COM1` and the rest open a device on Windows whatever their
/// extension or case.
fn windows_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        _ => {
            // Split on a character boundary: byte 3 can fall inside one.
            if !stem.is_char_boundary(3) {
                return false;
            }
            let (prefix, digit) = stem.split_at(3);
            matches!(prefix, "COM" | "LPT")
                && matches!(
                    digit,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileScope {
    root: RootKind,
    path: RelativePath,
    read: bool,
}

/// The enforcement form of a grant's filesystem scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Grants {
    /// Non-recursive scopes: each covers exactly one file or directory.
    exact: Vec<FileScope>,
    /// Recursive scopes: each covers a directory and everything beneath it.
    subtrees: Vec<FileScope>,
}

/// How a read scope covers a request's path. Maintainer decision,
/// 2026-10-20 (#857): a recursive read scope permits reading, listing and
/// searching anything beneath its directory; a non-recursive one covers one
/// object, which can be read if it is a file and listed if it is a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coverage {
    /// A non-recursive scope names exactly this path.
    Exact,
    /// The path is a recursive scope's directory or beneath it.
    Subtree,
}

impl Grants {
    /// Derives the enforcement form of `scopes`, which have already passed the
    /// profile's validation. A filesystem scope the service cannot enforce as
    /// written refuses the whole grant: narrowing it silently would leave the
    /// owner believing something was granted that never could be read.
    pub(crate) fn from_scopes(scopes: &[Value], policy: &ReadPolicy) -> Result<Self, Refusal> {
        let mut grants = Self {
            exact: Vec::new(),
            subtrees: Vec::new(),
        };
        for scope in scopes {
            let object = scope.as_object().ok_or(Refusal::GrantUnsupported)?;
            match object.get("kind").and_then(Value::as_str) {
                Some("filesystem") => {}
                Some(kind) if NON_FILESYSTEM_SCOPE_KINDS.contains(&kind) => continue,
                _ => return Err(Refusal::GrantUnsupported),
            }
            if object.len() != FILESYSTEM_SCOPE_FIELDS.len()
                || !FILESYSTEM_SCOPE_FIELDS
                    .iter()
                    .all(|field| object.contains_key(*field))
            {
                return Err(Refusal::GrantUnsupported);
            }
            let root = object
                .get("root")
                .and_then(Value::as_str)
                .and_then(RootKind::parse)
                .ok_or(Refusal::GrantUnsupported)?;
            let read = match object.get("access").and_then(Value::as_str) {
                Some("read") => true,
                Some("write") => false,
                _ => return Err(Refusal::GrantUnsupported),
            };
            let recursive = object
                .get("recursive")
                .and_then(Value::as_bool)
                .ok_or(Refusal::GrantUnsupported)?;
            let text = object
                .get("path")
                .and_then(Value::as_str)
                .ok_or(Refusal::GrantUnsupported)?;
            if too_broad(text, recursive) {
                return Err(Refusal::GrantUnsupported);
            }
            let path = RelativePath::parse(text, policy).map_err(|_| Refusal::GrantUnsupported)?;
            let scope = FileScope { root, path, read };
            if recursive {
                grants.subtrees.push(scope);
            } else {
                grants.exact.push(scope);
            }
        }
        Ok(grants)
    }

    /// How `request` is covered, if a read scope covers it for its operation.
    /// A scope covers only its own spelling: a case or Unicode alias of a
    /// granted name is not covered. Search needs a recursive scope.
    pub(crate) fn authorize(&self, request: &Request) -> Result<Coverage, Refusal> {
        let applies = |scope: &&FileScope| scope.root == request.root;
        let exact: Vec<&FileScope> = self
            .exact
            .iter()
            .filter(applies)
            .filter(|scope| scope.path == request.path)
            .collect();
        let subtrees: Vec<&FileScope> = self
            .subtrees
            .iter()
            .filter(applies)
            .filter(|scope| request.path.starts_with(&scope.path))
            .collect();
        let coverage = if subtrees.iter().any(|scope| scope.read) {
            Coverage::Subtree
        } else if exact.iter().any(|scope| scope.read) {
            Coverage::Exact
        } else if exact.is_empty() && subtrees.is_empty() {
            return Err(Refusal::ScopeMissing);
        } else {
            return Err(Refusal::ScopeAccessDenied);
        };
        if matches!(request.operation, Operation::Search { .. }) && coverage == Coverage::Exact {
            return Err(Refusal::ScopeOperationDenied);
        }
        Ok(coverage)
    }
}

/// The profile's own breadth rule (`validateScope`), applied again so that a
/// scope the service holds can never be broader than one the profile accepts.
fn too_broad(text: &str, recursive: bool) -> bool {
    let candidate = text.replace('\\', "/");
    let bytes = candidate.as_bytes();
    let drive =
        bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1..3) == Some(b":/");
    candidate == "."
        || candidate == "/"
        || candidate.contains('*')
        || candidate.contains("..")
        || candidate.starts_with('/')
        || drive
        || (recursive && candidate.split('/').filter(|part| !part.is_empty()).count() < 2)
}

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Operation {
    ReadFile,
    /// A directory's entries, continuing after the entry named `after`.
    ListDirectory {
        after: Option<String>,
    },
    /// Lines containing `pattern` in the files beneath a directory,
    /// continuing after the line at `after`.
    Search {
        pattern: String,
        after: Option<SearchPosition>,
    },
}

/// A line in a file, which is where a search continues from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchPosition {
    pub path: RelativePath,
    pub line: u64,
}

/// One request from the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub operation: Operation,
    pub root: RootKind,
    pub path: RelativePath,
}

impl Request {
    /// Parses a request object: `operation`, `root` and `path`, plus `after`
    /// to continue a listing or search and `pattern` for a search. Any other
    /// field refuses it. A continuation only says where to resume; each page
    /// is authorized afresh, so it carries no authority of its own.
    pub(crate) fn parse(value: &Value, policy: &ReadPolicy) -> Result<Self, Refusal> {
        let object = value.as_object().ok_or(Refusal::RequestInvalid)?;
        let optional: &[&str] = match object.get("operation").and_then(Value::as_str) {
            Some("read_file") => &[],
            Some("list_directory") => &["after"],
            Some("search") => &["pattern", "after"],
            _ => return Err(Refusal::RequestInvalid),
        };
        let required = REQUEST_FIELDS
            .iter()
            .chain(optional.iter().filter(|field| **field == "pattern"));
        if !required
            .into_iter()
            .all(|field| object.contains_key(*field))
            || !object.keys().all(|key| {
                REQUEST_FIELDS.contains(&key.as_str()) || optional.contains(&key.as_str())
            })
        {
            return Err(Refusal::RequestInvalid);
        }
        let root = object
            .get("root")
            .and_then(Value::as_str)
            .and_then(RootKind::parse)
            .ok_or(Refusal::RequestInvalid)?;
        let path = object
            .get("path")
            .and_then(Value::as_str)
            .ok_or(Refusal::RequestInvalid)?;
        let path = RelativePath::parse(path, policy)?;
        let operation = match object.get("operation").and_then(Value::as_str) {
            Some("list_directory") => Operation::ListDirectory {
                after: match object.get("after") {
                    None => None,
                    Some(after) => Some(
                        after
                            .as_str()
                            .filter(|name| RelativePath::is_name(name, policy))
                            .ok_or(Refusal::RequestInvalid)?
                            .to_owned(),
                    ),
                },
            },
            Some("search") => {
                let pattern = object
                    .get("pattern")
                    .and_then(Value::as_str)
                    .filter(|pattern| {
                        !pattern.is_empty()
                            && pattern.len() <= policy.pattern_bytes
                            // Lines split on newlines, so no other control
                            // character can be searched for usefully.
                            && !pattern.chars().any(|c| c.is_control() && c != '\t')
                    })
                    .ok_or(Refusal::RequestInvalid)?
                    .to_owned();
                let after = match object.get("after") {
                    None => None,
                    Some(after) => Some(search_position(after, &path, policy)?),
                };
                Operation::Search { pattern, after }
            }
            _ => Operation::ReadFile,
        };
        Ok(Self {
            operation,
            root,
            path,
        })
    }
}

/// A search continuation: a file strictly beneath the searched directory, and
/// a line in it.
fn search_position(
    value: &Value,
    searched: &RelativePath,
    policy: &ReadPolicy,
) -> Result<SearchPosition, Refusal> {
    let object = value.as_object().ok_or(Refusal::RequestInvalid)?;
    if object.len() != 2 {
        return Err(Refusal::RequestInvalid);
    }
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or(Refusal::RequestInvalid)?;
    let path = RelativePath::parse(path, policy).map_err(|_| Refusal::RequestInvalid)?;
    let line = object
        .get("line")
        .and_then(Value::as_u64)
        .filter(|line| *line >= 1)
        .ok_or(Refusal::RequestInvalid)?;
    if !path.starts_with(searched) || path == *searched {
        return Err(Refusal::RequestInvalid);
    }
    Ok(SearchPosition { path, line })
}
