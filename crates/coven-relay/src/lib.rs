//! The relay's production router, shared with endpoint integration fixtures.
use axum::{
    routing::{any, get},
    Router,
};

mod ws;

/// Each router owns an independent, bounded ephemeral rendezvous registry.
pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "OK" }))
        .route("/ws", any(ws::handler))
        .with_state(ws::RelayState::default())
}

use tracing_subscriber::{filter::filter_fn, fmt::MakeWriter, prelude::*, EnvFilter};

/// Restrict diagnostics to relay-owned events even when dependency tracing is enabled.
pub fn diagnostic_subscriber<W>(
    filter: EnvFilter,
    writer: W,
) -> impl tracing::Subscriber + Send + Sync
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
