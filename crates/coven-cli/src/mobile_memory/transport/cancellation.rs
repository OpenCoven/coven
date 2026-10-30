//! One-shot cancellation for a single outbound connection attempt.
use std::net::{Shutdown, TcpStream};
use std::sync::{Arc, Mutex};

use anyhow::{ensure, Result};

#[derive(Default)]
enum State {
    #[default]
    Fresh,
    Connecting(Option<Arc<mio::Waker>>),
    Connected(TcpStream, Option<Arc<mio::Waker>>),
    Finished,
    Cancelled(Option<Arc<mio::Waker>>),
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
        let _ = self.cancel_inner();
    }

    #[cfg(test)]
    pub(super) fn cancel_observed(&self) -> Option<std::io::Result<()>> {
        self.cancel_inner()
    }

    fn cancel_inner(&self) -> Option<std::io::Result<()>> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let shutdown = if let State::Connected(socket, _) = &*state {
            Some(socket.shutdown(Shutdown::Both))
        } else {
            None
        };
        if let State::Connecting(Some(waker)) | State::Connected(_, Some(waker)) = &*state {
            let _ = waker.wake();
        }
        // Keep the wake source alive until Attempt drops after its poller exits.
        // Dropping the last Waker here can discard a pending event on Linux.
        let previous = std::mem::replace(&mut *state, State::Cancelled(None));
        *state = match previous {
            State::Connecting(waker) | State::Connected(_, waker) | State::Cancelled(waker) => {
                State::Cancelled(waker)
            }
            _ => State::Cancelled(None),
        };
        shutdown
    }

    pub(super) fn ensure_active(&self) -> Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        ensure!(
            matches!(*state, State::Connected(_, _)),
            "transport attempt cancelled"
        );
        Ok(())
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
        self.cancellation.ensure_active()
    }

    pub(super) fn register_io_waker(&self, waker: Arc<mio::Waker>) -> Result<Cancellation> {
        let mut state = self
            .cancellation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("transport cancellation unavailable"))?;
        match &mut *state {
            State::Connected(_, registered) if registered.is_none() => *registered = Some(waker),
            _ => anyhow::bail!("transport attempt cancelled or I/O already registered"),
        }
        Ok(self.cancellation.clone())
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
            None,
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
        *state = if matches!(*state, State::Cancelled(_)) {
            State::Cancelled(None)
        } else {
            State::Finished
        };
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
    fn cancellation_retains_registered_waker_until_attempt_finishes() {
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        let poll = mio::Poll::new().unwrap();
        let waker = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
        let lifetime = Arc::downgrade(&waker);
        attempt.register_waker(waker).unwrap();
        cancellation.cancel();
        cancellation.cancel();
        // Mio only guarantees delivery while the registered waker remains alive.
        // Repeated cancellation must not close its eventfd before the poller exits.
        assert!(
            lifetime.upgrade().is_some(),
            "cancel dropped the pending wake source"
        );
        assert!(attempt.ensure_connecting().is_err());
        drop(attempt);
        assert!(
            lifetime.upgrade().is_none(),
            "finished attempt retained its wake source"
        );
        assert!(cancellation.begin().is_err());
    }

    #[test]
    fn connected_io_waker_survives_repeated_cancellation_until_attempt_finishes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        attempt.register(&socket).unwrap();
        let poll = mio::Poll::new().unwrap();
        let waker = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
        let lifetime = Arc::downgrade(&waker);
        attempt.register_io_waker(waker).unwrap();
        cancellation.cancel();
        cancellation.cancel();
        assert!(
            lifetime.upgrade().is_some(),
            "cancel dropped the connected wake source"
        );
        assert!(attempt.ensure_active().is_err());
        drop(attempt);
        assert!(
            lifetime.upgrade().is_none(),
            "finished attempt retained the wake source"
        );
    }

    #[test]
    fn cancellation_before_connected_waker_installation_refuses_io() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (_peer, _) = listener.accept().unwrap();
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        attempt.register(&socket).unwrap();
        cancellation.cancel();
        let poll = mio::Poll::new().unwrap();
        let waker = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1)).unwrap());
        let lifetime = Arc::downgrade(&waker);
        assert!(attempt.register_io_waker(waker).is_err());
        assert!(lifetime.upgrade().is_none());
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
