//! coven-relay — bounded opaque WebSocket rendezvous for OpenCoven devices.
//!
//! The relay is deliberately not an OpenCoven authority. It matches two peers
//! that know the same high-entropy room and token, then forwards binary frames.
//! Endpoint authentication, grants, and application encryption stay end to end.

use anyhow::Result;
use axum::{response::IntoResponse, routing::any, routing::get, Router};
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

    let relay = ws::RelayState::default();
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/ws", any(ws::handler))
        .with_state(relay);

    info!("coven-relay listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Health endpoint — `GET /healthz` → `200 OK`.
async fn healthz() -> impl IntoResponse {
    "OK"
}
