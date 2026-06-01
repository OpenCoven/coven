//! coven-relay — stateless WebSocket relay for Hexes.
//!
//! Phase 2A scaffold: minimal Axum WS server with a health endpoint.
//! Auth + peer routing (2B), offline buffering + APNs (2C), and avatar
//! serving (2D) are added in subsequent phases.

use anyhow::Result;
use axum::{Router, routing::get, response::IntoResponse};
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod ws;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let addr: SocketAddr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".into())
        .parse()?;

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", get(ws::handler));

    info!("coven-relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Health endpoint — `GET /healthz` → `200 OK`.
async fn healthz() -> impl IntoResponse {
    "OK"
}
