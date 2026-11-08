//! Live connection observations. These never confer or modify device authority.
use std::sync::{Arc, Mutex, Weak};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

// Ordering is observation priority across concurrent channels, not authority.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    #[default]
    Offline,
    Connecting,
    Authenticating,
    Online,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Relay,
    Direct,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    pub transport: Transport,
}

type LiveStatus = Mutex<ConnectionStatus>;

/// Contains no device IDs, endpoints, keys, grants or authorization state.
/// Weak observations cannot keep a failed or cancelled attempt online.
#[derive(Clone, Default)]
pub struct Connections(Arc<Mutex<Vec<Weak<LiveStatus>>>>);

impl Connections {
    pub fn begin(&self, transport: Transport) -> Result<Attempt> {
        let status = Arc::new(Mutex::new(ConnectionStatus {
            state: ConnectionState::Connecting,
            transport,
        }));
        let mut live = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("connection observations unavailable"))?;
        live.retain(|entry| entry.strong_count() > 0);
        live.push(Arc::downgrade(&status));
        Ok(Attempt { status })
    }

    pub fn snapshot(&self, transport: Transport) -> Result<ConnectionStatus> {
        let mut live = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("connection observations unavailable"))?;
        let mut state = ConnectionState::Offline;
        live.retain(|entry| entry.strong_count() > 0);
        for entry in live.iter().filter_map(Weak::upgrade) {
            let observed = entry
                .lock()
                .map_err(|_| anyhow::anyhow!("connection observation unavailable"))?;
            if observed.transport == transport {
                state = state.max(observed.state);
            }
        }
        Ok(ConnectionStatus { state, transport })
    }
}

pub struct Attempt {
    status: Arc<Mutex<ConnectionStatus>>,
}

impl Attempt {
    pub fn authenticating(&mut self) -> Result<()> {
        let mut status = self
            .status
            .lock()
            .ok()
            .context("connection observation unavailable")?;
        ensure!(
            status.state == ConnectionState::Connecting,
            "invalid connection transition"
        );
        status.state = ConnectionState::Authenticating;
        Ok(())
    }

    /// Call only after fresh endpoint proofs and current-authority admission.
    pub fn online(&mut self) -> Result<()> {
        let mut status = self
            .status
            .lock()
            .ok()
            .context("connection observation unavailable")?;
        ensure!(
            status.state == ConnectionState::Authenticating,
            "invalid connection transition"
        );
        status.state = ConnectionState::Online;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_tracks_proof_stages_and_returns_offline_on_error() {
        let connections = Connections::default();
        let mut attempt = connections.begin(Transport::Relay).unwrap();
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Connecting
        );
        assert!(attempt.online().is_err());
        attempt.authenticating().unwrap();
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Authenticating
        );
        attempt.online().unwrap();
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
        drop(attempt);
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Offline
        );
    }

    #[test]
    fn overlapping_attempts_do_not_hide_or_clear_an_authenticated_channel() {
        let connections = Connections::default();
        let mut established = connections.begin(Transport::Relay).unwrap();
        established.authenticating().unwrap();
        established.online().unwrap();
        let candidate = connections.begin(Transport::Relay).unwrap();
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
        drop(candidate);
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
        let mut replacement = connections.begin(Transport::Relay).unwrap();
        replacement.authenticating().unwrap();
        replacement.online().unwrap();
        drop(established);
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
        drop(replacement);
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Offline
        );
    }

    #[test]
    fn selected_transport_has_separate_state_and_only_coarse_serialized_fields() {
        let connections = Connections::default();
        let mut relay = connections.begin(Transport::Relay).unwrap();
        relay.authenticating().unwrap();
        relay.online().unwrap();
        let direct = connections.begin(Transport::Direct).unwrap();
        assert_eq!(
            connections.snapshot(Transport::Direct).unwrap().state,
            ConnectionState::Connecting
        );
        assert_eq!(
            serde_json::to_value(connections.snapshot(Transport::Relay).unwrap()).unwrap(),
            serde_json::json!({"state":"online", "transport":"relay"})
        );
        drop(direct);
        assert_eq!(
            connections.snapshot(Transport::Direct).unwrap().state,
            ConnectionState::Offline
        );
        assert_eq!(
            connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
    }
}
