//! Scoped reads for the Runtime Authority R0 envelope (coven#857).
//!
//! The design is `docs/architecture/coven-automations-scoped-read-service.md`:
//! the daemon, not the harness, interprets the granted filesystem scopes and
//! opens the files a model asks for. This module holds that contract's
//! operations: reading a file, listing a directory and searching the files
//! beneath one.
//!
//! - [`scope`] holds the lexical rules. It derives an enforcement form from
//!   validated profile scopes without rewriting them, parses a request under
//!   the same rules, and authorizes it before any filesystem access.
//! - [`open`] opens what a request names beneath a held root, one component at
//!   a time and never through a link. It refuses hard links, special files,
//!   mount crossings and protected resources, then rechecks from the root
//!   after the open so a concurrent rename cannot move it out of the root
//!   undetected. Listings and searches leave out every entry a read would
//!   refuse. Everything is bounded by the policy.
//!
//! - [`service`] is the MCP server the harness sees: three tools over one
//!   JSON-RPC stream, each call admitted and each response committed through
//!   the attempt's gate before a byte of it is written.
//! - [`channel`] is the private Unix socket between that server and the relay
//!   the harness launches, `coven automations scoped-read-relay`.
//!
//! - [`admission`] admits a launch and prepares its channel and folder. The
//!   daemon reaches it only through `automations::trusted_launch::prepare`,
//!   whose `StoreGate` (`automations::store_gate`) admits each call and
//!   commits each result under the store's write lock.

#[cfg(unix)]
pub(crate) mod admission;
#[cfg(unix)]
pub(crate) mod channel;
pub(crate) mod credential;
pub(crate) mod open;
pub(crate) mod scope;
pub(crate) mod service;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod tests;

/// Why a read was refused. Every refusal names one stable code, and a refused
/// read discloses no file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The request is not a closed object of known fields and types.
    RequestInvalid,
    /// A read scope covers the path, but not for this operation: search needs
    /// a recursive scope.
    ScopeOperationDenied,
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
    /// Only a recursive scope covers the path, and the path passes through a
    /// `.git` folder or a `.env` file beneath that scope's directory.
    ScopeExcluded,
    /// The file or one of its parent directories does not exist.
    ResourceNotFound,
    /// A component is a symbolic link.
    ResourceSymlink,
    /// A file was asked for, and this is a directory.
    ResourceDirectory,
    /// A directory was asked for, and this is not one.
    ResourceNotDirectory,
    /// The file is not UTF-8 text.
    ResourceNotText,
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
    /// The attempt has scanned all the names and file content it may.
    LimitScan,
    /// The operation ran past its time limit or the attempt's deadline.
    LimitTime,
    /// The attempt was cancelled, revoked or ended, so the service serves
    /// nothing more.
    AttemptClosed,
    /// This platform has no reviewed race-safe open for the service.
    PlatformUnsupported,
}

impl Refusal {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::RequestInvalid => "request_invalid",
            Self::ScopeOperationDenied => "scope_operation_denied",
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
            Self::ScopeExcluded => "scope_excluded",
            Self::ResourceNotFound => "resource_not_found",
            Self::ResourceSymlink => "resource_symlink",
            Self::ResourceDirectory => "resource_directory",
            Self::ResourceNotDirectory => "resource_not_directory",
            Self::ResourceNotText => "resource_not_text",
            Self::ResourceSpecial => "resource_special",
            Self::ResourceMultiplyLinked => "resource_multiply_linked",
            Self::ResourceMountCrossing => "resource_mount_crossing",
            Self::ResourceProtected => "resource_protected",
            Self::ResourceMoved => "resource_moved",
            Self::ResourceUnavailable => "resource_unavailable",
            Self::LimitFileTooLarge => "limit_file_too_large",
            Self::LimitAttemptDisclosure => "limit_attempt_disclosure",
            Self::LimitCalls => "limit_calls",
            Self::LimitScan => "limit_scan",
            Self::LimitTime => "limit_time",
            Self::AttemptClosed => "attempt_closed",
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
    /// The most file content one attempt discloses in total: file reads and
    /// matched search lines.
    pub attempt_bytes: u64,
    /// The most one attempt scans: directory entry names, and the content of
    /// every file a search reads.
    pub scan_bytes: u64,
    /// The most service calls one attempt makes, refused calls included.
    pub calls: u32,
    /// The most components a path may have.
    pub depth: usize,
    /// The most bytes a path may have.
    pub path_bytes: usize,
    /// The most entries one listing page carries.
    pub page_entries: usize,
    /// The most matches one search page carries.
    pub page_matches: usize,
    /// The most bytes a search pattern may have.
    pub pattern_bytes: usize,
    /// The most bytes of one matched line a search returns.
    pub match_bytes: usize,
}

/// The design's proposed limits for the R0 read envelope.
pub(crate) const R0_READ_POLICY: ReadPolicy = ReadPolicy {
    file_bytes: 1024 * 1024,
    attempt_bytes: 8 * 1024 * 1024,
    scan_bytes: 64 * 1024 * 1024,
    calls: 1024,
    depth: 32,
    path_bytes: 4096,
    page_entries: 256,
    page_matches: 256,
    pattern_bytes: 256,
    match_bytes: 512,
};
