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
/// The fields of a `read_file` request.
const READ_FILE_FIELDS: &[&str] = &["operation", "root", "path"];
/// Operations the design requires but no reviewed scope mapping covers yet.
const UNMAPPED_OPERATIONS: &[&str] = &["list_directory", "search"];

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
}

/// A relative path in the one spelling the service interprets: non-empty
/// components joined by `/`, none of them `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub(crate) fn components(&self) -> &[String] {
        &self.components
    }

    fn starts_with(&self, prefix: &Self) -> bool {
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
    /// Non-recursive scopes: each names one file.
    files: Vec<FileScope>,
    /// Recursive scopes. No reviewed mapping says which operations they
    /// authorize, so a request under one is refused as unmapped.
    subtrees: Vec<FileScope>,
}

impl Grants {
    /// Derives the enforcement form of `scopes`, which have already passed the
    /// profile's validation. A filesystem scope the service cannot enforce as
    /// written refuses the whole grant: narrowing it silently would leave the
    /// owner believing something was granted that never could be read.
    pub(crate) fn from_scopes(scopes: &[Value], policy: &ReadPolicy) -> Result<Self, Refusal> {
        let mut grants = Self {
            files: Vec::new(),
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
                grants.files.push(scope);
            }
        }
        Ok(grants)
    }

    /// Whether `request` may read its file. Only a non-recursive read scope
    /// naming exactly that path authorizes it: not its parent, not a sibling,
    /// and not another spelling of the same name.
    pub(crate) fn authorize(&self, request: &ReadRequest) -> Result<(), Refusal> {
        let mut named = self
            .files
            .iter()
            .filter(|scope| scope.root == request.root && scope.path == request.path)
            .peekable();
        if named.peek().is_some() {
            return if named.any(|scope| scope.read) {
                Ok(())
            } else {
                Err(Refusal::ScopeAccessDenied)
            };
        }
        if self
            .subtrees
            .iter()
            .any(|scope| scope.root == request.root && request.path.starts_with(&scope.path))
        {
            return Err(Refusal::OperationUnmapped);
        }
        Err(Refusal::ScopeMissing)
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

/// One `read_file` request from the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadRequest {
    pub root: RootKind,
    pub path: RelativePath,
}

impl ReadRequest {
    /// Parses a request object. An operation the design names but no reviewed
    /// mapping covers is refused as unmapped before its fields are read.
    pub(crate) fn parse(value: &Value, policy: &ReadPolicy) -> Result<Self, Refusal> {
        let object = value.as_object().ok_or(Refusal::RequestInvalid)?;
        match object.get("operation").and_then(Value::as_str) {
            Some("read_file") => {}
            Some(operation) if UNMAPPED_OPERATIONS.contains(&operation) => {
                return Err(Refusal::OperationUnmapped)
            }
            _ => return Err(Refusal::RequestInvalid),
        }
        if object.len() != READ_FILE_FIELDS.len()
            || !READ_FILE_FIELDS
                .iter()
                .all(|field| object.contains_key(*field))
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
        Ok(Self {
            root,
            path: RelativePath::parse(path, policy)?,
        })
    }
}
