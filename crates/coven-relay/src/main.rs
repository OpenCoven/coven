//! coven-relay — bounded opaque WebSocket rendezvous for OpenCoven devices.
//!
//! The relay is deliberately not an OpenCoven authority. It matches two peers
//! that know the same high-entropy room and token, then forwards binary frames.
//! Endpoint authentication, grants, and application encryption stay end to end.

use anyhow::Result;
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> Result<()> {
    coven_relay::diagnostic_subscriber(
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        std::io::stderr,
    )
    .init();

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;

    let app = coven_relay::router();

    info!("coven-relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
