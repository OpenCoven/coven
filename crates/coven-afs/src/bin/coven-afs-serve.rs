//! The export process the daemon spawns to back one AFS mount.
//!
//! This is daemon-internal plumbing, not a user-facing command. The daemon
//! owns the mount lifecycle; this process owns exactly one NFS export and dies
//! with it. Running the export out-of-process buys three things worth the
//! extra binary: `coven-cli` keeps its dependency surface (no tokio, no RPC
//! stack) on platforms that will never mount, a panicking export cannot take
//! the daemon down with it, and orphan recovery gets a real pid to sweep
//! rather than a thread to guess about.
//!
//! ```text
//! coven-afs-serve <delta-path> [base-path]
//! ```
//!
//! With a base, the export serves the merged base+delta view a mount is
//! supposed to show (DESIGN.md §3.2). Without one it serves the delta alone,
//! which is only useful for a session that has no base.
//!
//! **Handshake.** One line each on stdout, in order:
//!
//! ```text
//! port <port>
//! token <token>
//! ```
//!
//! **Control.** One command per line on stdin:
//!
//! ```text
//! rotate   -> replies `rotated`; the new token is never emitted
//! quit     -> exits 0
//! ```
//!
//! The parent needs the token exactly once, to build the `mount_nfs`
//! argument. After the mount returns it sends `rotate`, and the value it holds
//! — the one that was briefly `ps`-visible — stops opening the gate. The
//! parent never learns the replacement, because it has no further use for it.

use coven_afs::{AfsNfs, AgentFs, OverlayExport, OverlayFs};
use nfsserve::tcp::{NFSTcp, NFSTcpListener};

/// DESIGN.md §7 invariant: the export token is never logged, printed, or
/// displayed. Stdout here is a pipe to the daemon, which is a different thing
/// from a terminal — but only if it actually is one. Refusing a tty stdout
/// keeps a hand-run `coven-afs-serve` from painting a live credential into a
/// scrollback buffer or a captured CI log.
#[cfg(unix)]
fn refuse_terminal_stdout() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: isatty is a POSIX call on a borrowed fd with no memory effects.
    if unsafe { libc::isatty(libc::STDOUT_FILENO) } == 1 {
        return Err("coven-afs-serve writes an export token to stdout and \
                    refuses to run with a terminal attached; it is spawned by \
                    the daemon. For a hand-driven export, use the afs_serve \
                    example, which writes the token to a 0600 file."
            .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn refuse_terminal_stdout() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: coven-afs-serve <delta-path> [base-path]");
        std::process::exit(2);
    };
    let base = args.next();
    refuse_terminal_stdout()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Two shapes of export, one serving path. Splitting the listener setup
    // per branch would duplicate the handshake, which is the part that must
    // not drift.
    match base {
        Some(base) => {
            let overlay = OverlayFs::open(&path, &base)?;
            // A mounted overlay is scratch state whose durability story is
            // "discard or commit", so the WAL trade MOUNT-SPIKE.md §3 measured
            // is the right one. The daemon opens session deltas this way; a
            // delta being handed to someone else does not.
            overlay.delta().enable_wal()?;
            serve(AfsNfs::new(OverlayExport::new(overlay)?)).await
        }
        None => {
            let fs = AgentFs::create(&path)?;
            fs.enable_wal()?;
            serve(AfsNfs::new(fs)).await
        }
    }
}

/// Bind, hand the parent the port and token, and serve until told to stop.
async fn serve<F: coven_afs::ExportFs>(
    export: AfsNfs<F>,
) -> Result<(), Box<dyn std::error::Error>> {
    let gate = export.gate_handle();
    // Port 0: the OS picks, so the export never lands on a guessable port.
    let listener = NFSTcpListener::bind("127.0.0.1:0", export).await?;

    // Ordering matters: the port must be readable before the token, so a
    // parent that dies mid-handshake never leaves a token in a pipe buffer
    // belonging to an export nobody can reach.
    emit(&format!("port {}", listener.get_listen_port()))?;
    emit(&format!("token {}", gate.token()))?;

    // Control runs on a blocking thread: stdin is a pipe the parent may hold
    // open indefinitely, and the reactor should not be waiting on it.
    std::thread::spawn(move || control(gate));

    listener.handle_forever().await?;
    Ok(())
}

/// Write one handshake line and flush it. An unflushed handshake is a parent
/// blocked forever on a read.
fn emit(line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}")?;
    out.flush()
}

/// Serve control commands until stdin closes.
///
/// A closed stdin means the daemon is gone. Exiting then is what makes the
/// export's lifetime a subset of the daemon's: an orphaned export cannot
/// outlive the process that was accounting for it.
fn control(gate: coven_afs::GateHandle) {
    use std::io::BufRead as _;
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        match line.trim() {
            "rotate" => {
                gate.rotate();
                // The reply carries no value: the parent has no use for the
                // new token, and not sending it is one fewer copy in flight.
                if emit("rotated").is_err() {
                    break;
                }
            }
            "quit" => std::process::exit(0),
            "" => {}
            other => {
                eprintln!("coven-afs-serve: unknown command {other:?}");
            }
        }
    }
    // stdin closed.
    std::process::exit(0);
}
