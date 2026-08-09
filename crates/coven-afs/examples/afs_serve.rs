//! Serve an AgentFS database over loopback NFSv3.
//!
//! ```text
//! cargo run -p coven-afs --features mount --example afs_serve -- <db-path> [port]
//! ```
//!
//! Then, on macOS:
//!
//! ```text
//! mount_nfs -o vers=3,tcp,port=<port>,mountport=<port>,nolock,soft \
//!   localhost:/ <mountpoint>
//! ```
//!
//! Binds 127.0.0.1 only. Loopback NFS is unauthenticated, so anything that can
//! reach the port owns the export — see the trust note in `src/nfs.rs`.

use coven_afs::{AfsNfs, AgentFs};
use nfsserve::tcp::{NFSTcp, NFSTcpListener};

/// Ingest a host directory into the filesystem, the "base ingest" step a real
/// session would run before mounting.
fn import(fs: &mut AgentFs, root: &std::path::Path, prefix: &str) -> std::io::Result<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let child = format!("{}/{}", prefix.trim_end_matches('/'), name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            count += import(fs, &entry.path(), &child)?;
        } else if kind.is_file() {
            let data = std::fs::read(entry.path())?;
            fs.write_file(&child, &data)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            count += 1;
        }
    }
    Ok(count)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: afs_serve <db-path> [port]");
        std::process::exit(2);
    };
    // Port 0 asks the OS for a free port; the chosen one is printed below so a
    // caller can mount without a fixed, guessable port.
    let port: u16 = args.next().unwrap_or_else(|| "0".into()).parse()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .init();

    let mut fs = AgentFs::create(&path)?;
    fs.enable_wal()?;
    if let Ok(source) = std::env::var("AFS_IMPORT") {
        let started = std::time::Instant::now();
        let count = import(&mut fs, std::path::Path::new(&source), "")?;
        println!(
            "afs_serve: imported {count} files from {source} in {:.3} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
    let listener = NFSTcpListener::bind(&format!("127.0.0.1:{port}"), AfsNfs::new(fs)).await?;
    println!("afs_serve: port={}", listener.get_listen_port());
    listener.handle_forever().await?;
    Ok(())
}
