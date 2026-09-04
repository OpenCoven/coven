//! Transport-derived authority for daemon API requests.
//!
//! The local IPC transport is protected by filesystem permissions on Unix and
//! an owner-only named pipe on Windows. Loopback TCP guards reduce browser
//! risk, but they do not prove ownership of the daemon process.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestAuthority {
    OwnerLocalIpc,
    Tcp,
}

impl RequestAuthority {
    pub(crate) fn allows_session_launch_policy(self) -> bool {
        matches!(self, Self::OwnerLocalIpc)
    }

    pub(crate) fn allows_ward_proposal_access(self) -> bool {
        matches!(self, Self::OwnerLocalIpc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_capabilities_require_owner_local_ipc() {
        assert!(RequestAuthority::OwnerLocalIpc.allows_session_launch_policy());
        assert!(RequestAuthority::OwnerLocalIpc.allows_ward_proposal_access());
        assert!(!RequestAuthority::Tcp.allows_session_launch_policy());
        assert!(!RequestAuthority::Tcp.allows_ward_proposal_access());
    }
}
