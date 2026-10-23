//! The private channel between an attempt's scoped read service and the relay
//! the harness launches.
//!
//! Each attempt gets its own directory under `<COVEN_HOME>/runtime/scoped-read`,
//! readable only by the owner, holding a Unix socket and the MCP configuration
//! the harness is pointed at. The configuration names the relay
//! (`coven automations scoped-read-relay`) as the only MCP server and gives it
//! the socket's path and a random token in its environment, so neither
//! appears in any process's arguments.
//!
//! The channel accepts one connection that presents the token, then closes
//! its listener: nothing can join later, and a relay that disconnects cannot
//! reconnect. Closing the channel refuses every later call through the gate,
//! ends the connection and removes the files the channel created, one by
//! one; it never removes anything else.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::json;

use super::service::{Gate, Operation, Service, SERVER_NAME};
use super::Refusal;

/// The relay's arguments after the `coven` executable.
pub(crate) const RELAY_ARGS: &[&str] = &["automations", "scoped-read-relay"];
/// The relay's environment: the socket's path.
pub(crate) const CHANNEL_ENV: &str = "COVEN_SCOPED_READ_CHANNEL";
/// The relay's environment: the token it presents first.
pub(crate) const TOKEN_ENV: &str = "COVEN_SCOPED_READ_TOKEN";
/// How long a connection has to present the token.
const HANDSHAKE_TIME: Duration = Duration::from_secs(2);
/// How often the listener looks for a connection or a close.
const ACCEPT_POLL: Duration = Duration::from_millis(20);
/// The token's length in hex characters.
const TOKEN_HEX: usize = 64;
/// The most bytes a Unix socket path may have: `sun_path` holds 104 bytes on
/// macOS and 108 on Linux, including the terminating NUL.
#[cfg(target_os = "macos")]
const SOCKET_PATH_BYTES: usize = 103;
#[cfg(not(target_os = "macos"))]
const SOCKET_PATH_BYTES: usize = 107;

/// One attempt's open channel. Dropping it closes it.
pub(crate) struct Channel {
    directory: PathBuf,
    socket: PathBuf,
    config: PathBuf,
    closed: Arc<AtomicBool>,
    connection: Arc<Mutex<Option<UnixStream>>>,
    thread: Option<JoinHandle<Vec<Operation>>>,
}

impl Channel {
    /// Opens a channel for one attempt in a new directory beneath `runtime`,
    /// serving `service` once the relay connects. `relay` is the `coven`
    /// executable the harness launches.
    pub(crate) fn open(
        runtime: &Path,
        relay: &Path,
        build: impl FnOnce(Box<dyn Gate>) -> Service,
        gate: Box<dyn Gate>,
    ) -> Result<Self> {
        private_directory(runtime)?;
        let (directory, token) = loop {
            let name = random_hex(12)?;
            let directory = runtime.join(name);
            match DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => break (directory, random_hex(TOKEN_HEX / 2)?),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("creating scoped read channel {}", directory.display())
                    })
                }
            }
        };
        let socket = directory.join("s");
        let config = directory.join("mcp.json");
        let closed = Arc::new(AtomicBool::new(false));
        let mut channel = Self {
            directory,
            socket,
            config,
            closed: Arc::clone(&closed),
            connection: Arc::new(Mutex::new(None)),
            thread: None,
        };
        if channel.socket.as_os_str().len() > SOCKET_PATH_BYTES {
            bail!(
                "scoped read channel socket {} is longer than this platform allows",
                channel.socket.display()
            );
        }
        let listener = UnixListener::bind(&channel.socket)
            .with_context(|| format!("binding scoped read channel {}", channel.socket.display()))?;
        fs::set_permissions(&channel.socket, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        write_config(&channel.config, relay, &channel.socket, &token)?;

        let service = build(Box::new(Closing {
            closed: Arc::clone(&closed),
            inner: gate,
        }));
        let socket = channel.socket.clone();
        let connection = Arc::clone(&channel.connection);
        channel.thread = Some(
            thread::Builder::new()
                .name("scoped-read-channel".to_owned())
                .spawn(move || {
                    let Some(stream) = accept(&listener, &token, &closed) else {
                        return Vec::new();
                    };
                    // One connection only: nothing can join after the relay.
                    drop(listener);
                    let _ = fs::remove_file(&socket);
                    let Ok(held) = stream.try_clone() else {
                        return Vec::new();
                    };
                    *connection
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()) = Some(held);
                    // A close that ran before the connection was held has
                    // nothing to shut down, so look again.
                    if closed.load(Ordering::SeqCst) {
                        return Vec::new();
                    }
                    let Ok(input) = stream.try_clone() else {
                        return Vec::new();
                    };
                    let operations = service.run(BufReader::new(input), stream);
                    // The held handle would keep the relay's end open.
                    if let Some(held) = connection
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .take()
                    {
                        let _ = held.shutdown(std::net::Shutdown::Both);
                    }
                    operations
                })
                .context("starting the scoped read channel")?,
        );
        Ok(channel)
    }

    /// The MCP configuration the harness is launched with.
    pub(crate) fn config_path(&self) -> &Path {
        &self.config
    }

    /// Closes the channel and returns what each tool call did. Every later
    /// call is refused (`attempt_closed`). An operation already running
    /// finishes, but its result is withheld.
    pub(crate) fn close(mut self) -> Vec<Operation> {
        self.shut()
    }

    fn shut(&mut self) -> Vec<Operation> {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(stream) = self
            .connection
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take()
        {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        let operations = self
            .thread
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        // Only what the channel created, one entry at a time.
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_file(&self.config);
        let _ = fs::remove_dir(&self.directory);
        operations
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shut();
        }
    }
}

/// Refuses everything once the channel closes, then asks the attempt's own
/// gate.
struct Closing {
    closed: Arc<AtomicBool>,
    inner: Box<dyn Gate>,
}

impl Gate for Closing {
    fn admit(&mut self) -> Result<(), Refusal> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Refusal::AttemptClosed);
        }
        self.inner.admit()
    }

    fn commit(&mut self, tool: Option<&'static str>, digest: &str) -> Result<(), Refusal> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Refusal::AttemptClosed);
        }
        self.inner.commit(tool, digest)
    }
}

/// Waits for a connection that presents `token`, until the channel closes.
fn accept(listener: &UnixListener, token: &str, closed: &AtomicBool) -> Option<UnixStream> {
    while !closed.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                if handshake(&stream, token).unwrap_or(false) {
                    return Some(stream);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => return None,
        }
    }
    None
}

/// Reads the connection's first line, one byte at a time so nothing after it
/// is consumed, and compares it with `token` in constant time.
fn handshake(mut stream: &UnixStream, token: &str) -> io::Result<bool> {
    // An accepted socket inherits the listener's non-blocking mode on some
    // platforms.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIME))?;
    let mut line = Vec::with_capacity(TOKEN_HEX + 1);
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte)? == 0 {
            return Ok(false);
        }
        if byte[0] == b'\n' {
            break;
        }
        if line.len() == TOKEN_HEX {
            return Ok(false);
        }
        line.push(byte[0]);
    }
    stream.set_read_timeout(None)?;
    let expected = token.as_bytes();
    Ok(line.len() == expected.len()
        && line
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (got, want)| difference | (got ^ want))
            == 0)
}

/// Creates `path` if needed and makes sure it is a directory, not a link,
/// owned by this user and private to them.
fn private_directory(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_err() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .with_context(|| format!("creating {}", path.display()))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid() only reads the calling process's effective uid.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "refusing scoped read runtime {}: not a directory owned by this user",
            path.display()
        );
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_config(path: &Path, relay: &Path, socket: &Path, token: &str) -> Result<()> {
    let text = |path: &Path| {
        path.to_str()
            .with_context(|| format!("{} is not UTF-8", path.display()))
            .map(str::to_owned)
    };
    let (relay, socket) = (text(relay)?, text(socket)?);
    let config = json!({
        "mcpServers": {
            SERVER_NAME: {
                "type": "stdio",
                "command": relay,
                "args": RELAY_ARGS,
                "env": {
                    CHANNEL_ENV: socket,
                    TOKEN_ENV: token,
                },
            },
        },
    });
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    file.write_all(&serde_json::to_vec(&config)?)?;
    file.sync_all()?;
    Ok(())
}

fn random_hex(bytes: usize) -> Result<String> {
    use ring::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut buffer)
        .map_err(|_| anyhow::anyhow!("no system randomness for the scoped read channel"))?;
    Ok(buffer.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The relay: connects to the channel at `socket`, presents `token`, then
/// copies `input` to the channel and the channel to `output` until the
/// channel ends.
pub(crate) fn relay(
    socket: &Path,
    token: &str,
    mut input: impl Read + Send + 'static,
    mut output: impl Write,
) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(token.as_bytes())?;
    stream.write_all(b"\n")?;
    let mut upstream = stream.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut input, &mut upstream);
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        output.write_all(&buffer[..read])?;
        output.flush()?;
    }
}
