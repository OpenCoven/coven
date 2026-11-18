//! Host commands that use the owner's daemon or the independent peer service.
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use coven_client::{
    host::{HostConnect, HostInit, HostOperation, HostResolution, HostResolve, HostRun},
    DaemonClient, DaemonEndpoint,
};
use serde_json::Value;
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Args, Debug)]
pub(crate) struct Peer {
    #[arg(long, default_value = "http://127.0.0.1:8765")]
    url: String,
    #[arg(long)]
    token_file: PathBuf,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Initialize the local folder (defaults to ~/Coven).
    Init {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Configure a peer and reference to its protected token file.
    Connect {
        url: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Reconcile once and wait for the result.
    Sync,
    /// Show current folder, pending work, conflicts and recovery paths.
    Status,
    /// Follow the daemon's polling loop until interrupted.
    Watch,
    /// Choose the bytes to keep for a conflicted Castle path.
    Resolve {
        path: String,
        #[command(flatten)]
        choice: Choice,
    },
    /// Run a loopback peer without a Coven daemon or desktop session.
    Serve {
        #[arg(long)]
        storage: Option<PathBuf>,
        #[arg(long, default_value_t = 8765)]
        port: u16,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Upload a source file to the peer.
    Put {
        #[command(flatten)]
        peer: Peer,
        path: String,
        source: PathBuf,
    },
    /// Write the peer file's exact bytes to stdout.
    Get {
        #[command(flatten)]
        peer: Peer,
        path: String,
    },
    /// Publish the example result to the peer.
    DemoResult {
        #[command(flatten)]
        peer: Peer,
    },
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub(crate) struct Choice {
    #[arg(long)]
    local: bool,
    #[arg(long)]
    remote: bool,
    #[arg(long, value_name = "FILE")]
    merged: Option<PathBuf>,
}

fn defaults(relative: &str) -> Result<PathBuf> {
    Ok(dirs_next::home_dir()
        .context("user home directory is unavailable")?
        .join(relative))
}

fn peer_command(
    command: &str,
    peer: Peer,
    path: Option<&str>,
    source: Option<&Path>,
) -> Result<()> {
    let bytes = coven_host::peer_command(command, &peer.url, &peer.token_file, path, source)?;
    std::io::stdout().lock().write_all(&bytes)?;
    Ok(())
}

fn daemon_client(home: &Path) -> Result<DaemonClient> {
    if let Ok(endpoint) = DaemonEndpoint::discover(home) {
        let mut client = DaemonClient::new(endpoint);
        match client.health() {
            Ok(_) => return Ok(client),
            // A crashed daemon can leave a discoverable socket. Only a failed
            // connect proves it is absent. Preserve live peers' protocol and
            // capability errors, and never replay a host mutation.
            Err(ref err @ coven_client::ClientError::Io { ref source, .. })
                if err.request_was_definitely_not_sent()
                    && matches!(
                        source.kind(),
                        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                    ) => {}
            Err(err) => return Err(err.into()),
        }
    }
    crate::daemon::ensure_background_server(
        home,
        &std::env::current_exe()?,
        crate::current_timestamp(),
    )?;
    Ok(DaemonClient::new(DaemonEndpoint::discover(home)?))
}

pub(crate) fn run(command: Command) -> Result<()> {
    use Command::*;
    let op = match command {
        Serve {
            storage,
            port,
            token_file,
        } => {
            let storage = match storage {
                Some(storage) => storage,
                None => defaults(".local/state/coven-host/server")?,
            };
            return coven_host::peer::serve(
                &storage,
                port,
                coven_host::folder::token_from(&token_file)?,
            );
        }
        Put { peer, path, source } => return peer_command("put", peer, Some(&path), Some(&source)),
        Get { peer, path } => return peer_command("get", peer, Some(&path), None),
        DemoResult { peer } => return peer_command("demo-result", peer, None, None),
        Init { root } => HostOperation::Init(HostInit {
            root: match root {
                Some(p) => std::path::absolute(p)?,
                None => defaults("Coven")?,
            },
        }),
        Connect { url, token_file } => HostOperation::Connect(HostConnect {
            url,
            token_file: std::path::absolute(token_file)?,
        }),
        Resolve { path, choice } => {
            let resolution = if let Some(file) = choice.merged {
                HostResolution::Merged(std::path::absolute(file)?)
            } else if choice.local {
                HostResolution::Local
            } else {
                HostResolution::Remote
            };
            HostOperation::Resolve(HostResolve { path, resolution })
        }
        Status => HostOperation::Status,
        Sync => HostOperation::Sync,
        Watch => HostOperation::Watch,
    };
    let mut client = daemon_client(&crate::paths::coven_home_dir()?)?;
    match op {
        HostOperation::Sync | HostOperation::Resolve(_) => {
            let sync = matches!(op, HostOperation::Sync);
            let run: HostRun = serde_json::from_value(client.host(op)?)?;
            loop {
                let reply = client.host(HostOperation::Result(HostRun { id: run.id.clone() }))?;
                if !reply.is_null() {
                    println!("{}", serde_json::to_string_pretty(&reply)?);
                    if sync && reply["phase"] != "synced" {
                        bail!(
                            "{}",
                            reply["error"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .or_else(|| reply["phase"].as_str())
                                .unwrap_or("sync failed")
                        );
                    }
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        HostOperation::Watch => {
            let mut previous = Value::Null;
            loop {
                let next = client.host(HostOperation::Watch)?;
                if next != previous {
                    println!("{}", serde_json::to_string(&next)?);
                    std::io::stdout().flush()?;
                    previous = next;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(&client.host(op)?)?);
            Ok(())
        }
    }
}
