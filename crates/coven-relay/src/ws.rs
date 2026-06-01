//! WebSocket handler — Phase 2A stub.
//!
//! Right now this accepts the upgrade and immediately closes. Auth + peer
//! routing land in Phase 2B.

use axum::{
    extract::WebSocketUpgrade,
    response::IntoResponse,
};

pub async fn handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(|_socket| async move {
        // Phase 2B: authenticate bearer token, register peer role (host/client),
        // fan-out messages between peers sharing the same secret.
        // Socket drops here, closing the connection cleanly.
    })
}
