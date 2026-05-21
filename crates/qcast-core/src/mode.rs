//! Connection modes. Both are always available; they differ only in the
//! signaling transport and the ICE configuration. The media pipeline is the
//! same for both.

use serde::{Deserialize, Serialize};

/// Which transport path a session uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    /// Direct on the local network: host ICE candidates only, no STUN/TURN,
    /// no server. Lowest latency; works even when the private server is down.
    Lan,
    /// Through the private server: remote WebSocket signaling + STUN/TURN relay.
    Web,
}

impl ConnectionMode {
    /// Whether this mode requires the private server to be reachable.
    pub fn needs_server(self) -> bool {
        matches!(self, ConnectionMode::Web)
    }
}

/// A STUN or TURN server entry (Web mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub uri: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub credential: Option<String>,
}

/// ICE configuration derived from the connection mode and passed to `webrtcbin`.
#[derive(Debug, Clone, Default)]
pub struct IceConfig {
    /// STUN/TURN servers. Empty in LAN mode.
    pub servers: Vec<IceServer>,
    /// Force relay-only candidates (Web-mode option for maximum NAT predictability).
    pub force_relay: bool,
}

impl IceConfig {
    /// LAN mode: no ICE servers, host candidates only.
    pub fn lan() -> Self {
        Self::default()
    }

    /// Web mode: use the supplied STUN/TURN servers.
    pub fn web(servers: Vec<IceServer>, force_relay: bool) -> Self {
        Self { servers, force_relay }
    }
}
