//! coven-relay — bounded opaque WebSocket rendezvous for OpenCoven devices.
//!
//! The relay is deliberately not an OpenCoven authority. It matches two peers
//! that know the same high-entropy room and token, then forwards binary frames.
//! Endpoint authentication, grants, and application encryption stay end to end.

use anyhow::Result;
use axum::{response::IntoResponse, routing::any, routing::get, Router};
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::{filter::filter_fn, fmt::MakeWriter, prelude::*, EnvFilter};

fn diagnostic_subscriber<W>(filter: EnvFilter, writer: W) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'a> MakeWriter<'a> + Send + Sync + 'static,
{
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_filter(filter)
            // Dependency trace/debug events can include HTTP credentials and
            // WebSocket payloads. RUST_LOG must never enable those events.
            .with_filter(filter_fn(|metadata| {
                metadata.target() == "coven_relay" || metadata.target().starts_with("coven_relay::")
            })),
    )
}

mod ws;

#[tokio::main]
async fn main() -> Result<()> {
    diagnostic_subscriber(
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        std::io::stderr,
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
