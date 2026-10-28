//! Bounded resolver admission and cancellable nonblocking TCP establishment.
use super::cancellation::{Attempt, Cancellation};
use anyhow::{ensure, Result};
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Arc, LazyLock,
};
use std::time::{Duration, Instant};

const WAIT_SLICE: Duration = Duration::from_millis(50);
const MAX_ADDRESSES: usize = 16;
const MAX_RESOLVERS: usize = 4;
static RESOLVERS: LazyLock<Arc<AtomicUsize>> = LazyLock::new(|| Arc::new(AtomicUsize::new(0)));

pub(super) fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "transport deadline expired"))
}

struct ResolverPermit(Arc<AtomicUsize>);
impl Drop for ResolverPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn resolve(
    host: String,
    port: u16,
    cancellation: &Cancellation,
    deadline: Instant,
) -> Result<Vec<SocketAddr>> {
    ensure!(
        !host.is_empty() && host.len() <= 253 && port != 0,
        "invalid relay lookup"
    );
    resolve_with(
        Arc::clone(&RESOLVERS),
        MAX_RESOLVERS,
        cancellation,
        deadline,
        move || {
            (host.as_str(), port)
                .to_socket_addrs()
                .map(|addresses| addresses.take(MAX_ADDRESSES).collect())
        },
    )
}

fn resolve_with<F>(
    active: Arc<AtomicUsize>,
    capacity: usize,
    cancellation: &Cancellation,
    deadline: Instant,
    lookup: F,
) -> Result<Vec<SocketAddr>>
where
    F: FnOnce() -> io::Result<Vec<SocketAddr>> + Send + 'static,
{
    cancellation.ensure_pending()?;
    remaining(deadline)?;
    let (receiver, _worker) = spawn_lookup(active, capacity, lookup)?;
    wait_lookup(receiver, cancellation, deadline)
}

type LookupReceiver = mpsc::Receiver<io::Result<Vec<SocketAddr>>>;

fn spawn_lookup<F>(
    active: Arc<AtomicUsize>,
    capacity: usize,
    lookup: F,
) -> Result<(LookupReceiver, std::thread::JoinHandle<()>)>
where
    F: FnOnce() -> io::Result<Vec<SocketAddr>> + Send + 'static,
{
    active
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < capacity).then_some(count + 1)
        })
        .map_err(|_| anyhow::anyhow!("relay resolver busy"))?;
    let permit = ResolverPermit(active);
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("coven-relay-dns".into())
        .spawn(move || {
            let result = lookup();
            drop(permit);
            let _ = sender.send(result);
        })
        .map_err(|_| anyhow::anyhow!("relay resolver unavailable"))?;
    Ok((receiver, worker))
}

fn wait_lookup(
    receiver: LookupReceiver,
    cancellation: &Cancellation,
    deadline: Instant,
) -> Result<Vec<SocketAddr>> {
    loop {
        cancellation.ensure_pending()?;
        let wait = remaining(deadline)?.min(WAIT_SLICE);
        match receiver.recv_timeout(wait) {
            Ok(result) => {
                cancellation.ensure_pending()?;
                remaining(deadline)?;
                let addresses = result.map_err(|_| anyhow::anyhow!("relay lookup failed"))?;
                let mut bounded = Vec::new();
                for address in addresses.into_iter().take(MAX_ADDRESSES) {
                    if !bounded.contains(&address) {
                        bounded.push(address);
                    }
                }
                ensure!(!bounded.is_empty(), "relay lookup returned no addresses");
                return Ok(bounded);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("relay resolver unavailable")
            }
        }
    }
}

pub(super) fn connect(
    address: SocketAddr,
    attempt: &Attempt,
    deadline: Instant,
) -> Result<TcpStream> {
    let result = connect_inner(address, attempt, deadline);
    result.map_err(|_| anyhow::anyhow!("relay connection failed or cancelled"))
}

fn connect_inner(address: SocketAddr, attempt: &Attempt, deadline: Instant) -> Result<TcpStream> {
    attempt.ensure_connecting()?;
    remaining(deadline)?;
    let mut poll = mio::Poll::new()?;
    let waker = Arc::new(mio::Waker::new(poll.registry(), mio::Token(1))?);
    attempt.register_waker(waker)?;
    attempt.ensure_connecting()?;
    let mut socket = mio::net::TcpStream::connect(address)?;
    poll.registry()
        .register(&mut socket, mio::Token(0), mio::Interest::WRITABLE)?;
    let mut events = mio::Events::with_capacity(2);
    loop {
        attempt.ensure_connecting()?;
        match poll.poll(&mut events, Some(remaining(deadline)?.min(WAIT_SLICE))) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        }
        attempt.ensure_connecting()?;
        remaining(deadline)?;
        if !events.iter().any(|event| event.token() == mio::Token(0)) {
            continue;
        }
        if let Some(error) = socket.take_error()? {
            return Err(error.into());
        }
        match socket.peer_addr() {
            Ok(_) => break,
            Err(error) if connect_pending(&error) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    poll.registry().deregister(&mut socket)?;
    let socket: TcpStream = socket.into();
    socket.set_nonblocking(false)?;
    attempt.register(&socket)?;
    Ok(socket)
}

fn connect_pending(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::NotConnected | io::ErrorKind::WouldBlock
    ) {
        return true;
    }
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::EINPROGRESS) {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_lookup_retains_capacity_until_system_resolver_finishes() {
        let active = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let (receiver, resolver) = spawn_lookup(Arc::clone(&active), 1, move || {
            released.recv().unwrap();
            Ok(vec!["127.0.0.1:443".parse().unwrap()])
        })
        .unwrap();
        assert_eq!(active.load(Ordering::Acquire), 1);
        let cancellation = Cancellation::default();
        let waiting_token = cancellation.clone();
        let (done, completed) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let result = wait_lookup(
                receiver,
                &waiting_token,
                Instant::now() + Duration::from_secs(120),
            );
            done.send(result.is_err()).unwrap();
        });
        cancellation.cancel();
        // Hang guard only: the resolver remains deliberately blocked until after
        // the waiter has exited. Cancellation, not DNS completion, must end waiting.
        assert!(completed.recv_timeout(Duration::from_secs(30)).unwrap());
        waiter.join().unwrap();
        assert!(spawn_lookup(Arc::clone(&active), 1, || panic!("capacity exceeded")).is_err());
        assert_eq!(active.load(Ordering::Acquire), 1);
        release.send(()).unwrap();
        resolver.join().unwrap();
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert_eq!(
            resolve_with(
                active,
                1,
                &Cancellation::default(),
                Instant::now() + Duration::from_secs(30),
                || Ok(vec!["127.0.0.1:443".parse().unwrap()])
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn expired_or_cancelled_lookup_never_spawns_work() {
        let active = Arc::new(AtomicUsize::new(0));
        let cancellation = Cancellation::default();
        assert!(resolve_with(
            Arc::clone(&active),
            1,
            &cancellation,
            Instant::now(),
            || panic!("expired lookup spawned")
        )
        .is_err());
        cancellation.cancel();
        assert!(resolve_with(
            Arc::clone(&active),
            1,
            &cancellation,
            Instant::now() + Duration::from_secs(30),
            || panic!("cancelled lookup spawned")
        )
        .is_err());
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn lookup_timeout_keeps_its_permit_until_the_producer_finishes() {
        let active = Arc::new(AtomicUsize::new(0));
        let (release, released) = mpsc::channel();
        let (receiver, resolver) = spawn_lookup(Arc::clone(&active), 1, move || {
            released.recv().unwrap();
            Ok(vec!["127.0.0.1:443".parse().unwrap()])
        })
        .unwrap();
        let error = wait_lookup(receiver, &Cancellation::default(), Instant::now()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(active.load(Ordering::Acquire), 1);
        release.send(()).unwrap();
        resolver.join().unwrap();
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn lookup_results_are_bounded_and_duplicates_do_not_add_candidates() {
        let result = resolve_with(
            Arc::new(AtomicUsize::new(0)),
            1,
            &Cancellation::default(),
            Instant::now() + Duration::from_secs(30),
            || {
                Ok((1..=100)
                    .map(|port| SocketAddr::from(([127, 0, 0, 1], port)))
                    .collect())
            },
        )
        .unwrap();
        assert_eq!(result.len(), MAX_ADDRESSES);
        let result = resolve_with(
            Arc::new(AtomicUsize::new(0)),
            1,
            &Cancellation::default(),
            Instant::now() + Duration::from_secs(30),
            || Ok(vec!["127.0.0.1:443".parse().unwrap(); 16]),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn cancellation_wakes_the_pending_connection_poller() {
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        let mut poll = mio::Poll::new().unwrap();
        attempt
            .register_waker(Arc::new(
                mio::Waker::new(poll.registry(), mio::Token(1)).unwrap(),
            ))
            .unwrap();
        let (done, completed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut events = mio::Events::with_capacity(2);
            poll.poll(&mut events, Some(Duration::from_secs(120)))
                .unwrap();
            assert!(events.iter().any(|event| event.token() == mio::Token(1)));
            done.send(attempt.ensure_connecting().is_err()).unwrap();
        });
        cancellation.cancel();
        assert!(completed.recv_timeout(Duration::from_secs(30)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn connector_uses_nonblocking_completion_then_returns_blocking_stream() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cancellation = Cancellation::default();
        let attempt = cancellation.begin().unwrap();
        let mut socket = connect(
            listener.local_addr().unwrap(),
            &attempt,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        socket.write_all(b"connected").unwrap();
        let mut bytes = [0; 9];
        peer.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"connected");
        cancellation.cancel();
        assert!(attempt.ensure_active().is_err());
        assert_eq!(peer.read(&mut bytes).unwrap(), 0);
    }
}
