//! One-shot cancellation for a single outbound connection attempt.
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

use anyhow::{ensure, Result};

#[derive(Default)]
enum State {
    #[default]
    Fresh,
    Connecting(Option<Arc<mio::Waker>>),
    Connected(TcpStream),
    Finished,
    Cancelled,
}

/// Clones refer to the same attempt. Create a new token for every retry.
#[derive(Clone, Default)]
pub struct Cancellation {
    state: Arc<Mutex<State>>,
}

impl Cancellation {
    /// Interrupt TLS, HTTP upgrade, and record reads/writes on the active socket.
    /// Wake a pending TCP connection as well as interrupting established I/O.
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let State::Connected(socket) = &*state {
            let _ = socket.shutdown(Shutdown::Both);
        }
        if let State::Connecting(Some(waker)) = &*state {
            let _ = waker.wake();
        }
        *state = State::Cancelled;
    }

    pub(super) fn ensure_pending(&self) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Fresh),
            "transport attempt unavailable"
        );
        Ok(())
    }

    pub(super) fn begin(&self) -> Result<Attempt> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Fresh),
            "transport attempt unavailable"
        );
        *state = State::Connecting(None);
        Ok(Attempt {
            cancellation: self.clone(),
        })
    }
}

pub(super) struct Attempt {
    cancellation: Cancellation,
}

impl Attempt {
    pub(super) fn ensure_connecting(&self) -> Result<()> {
        let state = self
            .cancellation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Connecting(_)),
            "transport attempt cancelled"
        );
        Ok(())
    }

    pub(super) fn register_waker(&self, waker: Arc<mio::Waker>) -> Result<()> {
        let mut state = self
            .cancellation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Connecting(_)),
            "transport attempt cancelled"
        );
        *state = State::Connecting(Some(waker));
        Ok(())
    }

    pub(super) fn ensure_active(&self) -> Result<()> {
        let state = self
            .cancellation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Connected(_)),
            "transport attempt cancelled"
        );
        Ok(())
    }

    pub(super) fn register(&self, socket: &TcpStream) -> Result<()> {
        let mut state = self
            .cancellation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        if !matches!(*state, State::Connecting(_)) {
            let _ = socket.shutdown(Shutdown::Both);
            anyhow::bail!("transport attempt cancelled");
        }
        *state = State::Connected(
            socket
                .try_clone()
                .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?,
        );
        Ok(())
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        let mut state = self
            .cancellation
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, State::Cancelled) {
            *state = State::Finished;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn cancellation_before_registration_closes_late_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        cancellation.cancel();
        assert!(attempt.register(&socket).is_err());
        assert!(attempt.ensure_active().is_err());
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
        drop(attempt);
        assert!(cancellation.begin().is_err());
    }

    #[test]
    fn tokens_cannot_be_reused_or_claimed_concurrently() {
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        assert!(cancellation.begin().is_err());
        drop(attempt);
        assert!(cancellation.begin().is_err());
        cancellation.cancel();
        assert!(cancellation.begin().is_err());
    }
}
