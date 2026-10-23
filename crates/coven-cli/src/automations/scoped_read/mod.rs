//! Scoped reads for the Runtime Authority R0 envelope (coven#857).
//!
//! The design is `docs/architecture/coven-automations-scoped-read-service.md`:
//! the daemon, not the harness, interprets the granted filesystem scopes and
//! opens the files a model asks for. This module is that contract's first
//! increment, the exact-file read:
//!
//! - [`scope`] holds the lexical rules. It derives an enforcement form from
//!   validated profile scopes without rewriting them, parses a read request
//!   under the same rules, and authorizes it before any filesystem access.
//! - [`open`] opens an authorized file beneath a held root, one component at a
//!   time and never through a link. It refuses hard links, special files, mount
//!   crossings and protected resources, then rechecks from the root after the
//!   open so a concurrent rename cannot move the file out of the root
//!   undetected. It reads at most the policy's bytes.
//!
//! Directory listing and content search stay refused until the upstream scope
//! vocabulary maps them (the design's "Separate capability, access, resource,
//! and operation"). Nothing launches this yet: the service channel, harness
//! admission and response commitment are later steps of the same design.

pub(crate) mod open;
pub(crate) mod scope;
#[cfg(test)]
mod tests;

/// Why a read was refused. Every refusal names one stable code, and a refused
/// read discloses no file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The request is not a closed object of known fields and types.
    RequestInvalid,
    /// The operation is known but no reviewed scope mapping authorizes it yet.
    OperationUnmapped,
    /// The path is absolute or drive-qualified.
    PathAbsolute,
    /// The path has a `..` component.
    PathTraversal,
    /// The path uses a spelling the service does not interpret: empty, a
    /// backslash, a repeated or trailing separator, a `.` component, a colon,
    /// a control character, or a name Windows aliases.
    PathEncodingUnsupported,
    /// The path is longer or deeper than the policy allows.
    PathLimit,
    /// A filesystem scope in the grant cannot be enforced as written.
    GrantUnsupported,
    /// No root of the requested kind is bound to this attempt.
    RootUnbound,
    /// A root is, or is inside, a protected resource.
    RootProtected,
    /// The root's path no longer names the directory bound at the start.
    RootMoved,
    /// No read scope names this file.
    ScopeMissing,
    /// A scope names this file, but not for reading.
    ScopeAccessDenied,
    /// The file or one of its parent directories does not exist.
    ResourceNotFound,
    /// A component is a symbolic link.
    ResourceSymlink,
    /// The file is a directory.
    ResourceDirectory,
    /// The file is a FIFO, socket or device.
    ResourceSpecial,
    /// The file has more than one hard link.
    ResourceMultiplyLinked,
    /// A component is on another filesystem than the root.
    ResourceMountCrossing,
    /// A component is a protected resource.
    ResourceProtected,
    /// A component changed between the open and the recheck from the root.
    ResourceMoved,
    /// The filesystem returned another error.
    ResourceUnavailable,
    /// The file is larger than one response may carry.
    LimitFileTooLarge,
    /// The read would pass the attempt's total disclosure.
    LimitAttemptDisclosure,
    /// The attempt has used all of its service calls.
    LimitCalls,
    /// This platform has no reviewed race-safe open for the service.
    PlatformUnsupported,
}

impl Refusal {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::RequestInvalid => "request_invalid",
            Self::OperationUnmapped => "operation_unmapped",
            Self::PathAbsolute => "path_absolute",
            Self::PathTraversal => "path_traversal",
            Self::PathEncodingUnsupported => "path_encoding_unsupported",
            Self::PathLimit => "path_limit",
            Self::GrantUnsupported => "grant_unsupported",
            Self::RootUnbound => "root_unbound",
            Self::RootProtected => "root_protected",
            Self::RootMoved => "root_moved",
            Self::ScopeMissing => "scope_missing",
            Self::ScopeAccessDenied => "scope_access_denied",
            Self::ResourceNotFound => "resource_not_found",
            Self::ResourceSymlink => "resource_symlink",
            Self::ResourceDirectory => "resource_directory",
            Self::ResourceSpecial => "resource_special",
            Self::ResourceMultiplyLinked => "resource_multiply_linked",
            Self::ResourceMountCrossing => "resource_mount_crossing",
            Self::ResourceProtected => "resource_protected",
            Self::ResourceMoved => "resource_moved",
            Self::ResourceUnavailable => "resource_unavailable",
            Self::LimitFileTooLarge => "limit_file_too_large",
            Self::LimitAttemptDisclosure => "limit_attempt_disclosure",
            Self::LimitCalls => "limit_calls",
            Self::PlatformUnsupported => "platform_unsupported",
        }
    }
}

/// The bounds one attempt reads under. They belong to the envelope descriptor,
/// never to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadPolicy {
    /// The most one file response carries.
    pub file_bytes: u64,
    /// The most file content one attempt discloses in total.
    pub attempt_bytes: u64,
    /// The most service calls one attempt makes, refused calls included.
    pub calls: u32,
    /// The most components a path may have.
    pub depth: usize,
    /// The most bytes a path may have.
    pub path_bytes: usize,
}

/// The design's proposed limits for the R0 read envelope.
pub(crate) const R0_READ_POLICY: ReadPolicy = ReadPolicy {
    file_bytes: 1024 * 1024,
    attempt_bytes: 8 * 1024 * 1024,
    calls: 1024,
    depth: 32,
    path_bytes: 4096,
};
