//! Qcast signaling server (Web mode): a WebSocket relay that brokers SDP/ICE
//! between peers and authenticates them. Pairs with a TURN relay for media relay.
//!
//! LAN mode does not use this server at all.
//!
//! Deferred: Web-mode signalling server (not yet implemented). Planned transport
//! is WebSocket signaling (axum + tokio-tungstenite) with token auth.

fn main() {
    tracing_subscriber::fmt::init();
    println!("qcast-server: Web-mode signalling server is not yet implemented");
}
