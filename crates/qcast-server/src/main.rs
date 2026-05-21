//! Qcast signaling server (Web mode): a WebSocket relay that brokers SDP/ICE
//! between peers and authenticates them. Pairs with coturn for media relay.
//!
//! LAN mode does not use this server at all.
//!
//! TODO(Phase 2): WebSocket signaling (axum + tokio-tungstenite) + token auth.

fn main() {
    tracing_subscriber::fmt::init();
    println!("qcast-server: Web-mode signaling - implemented in Phase 2");
}
