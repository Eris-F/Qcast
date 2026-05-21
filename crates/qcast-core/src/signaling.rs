//! The signaling protocol: messages two peers exchange to establish a WebRTC
//! connection. The same messages flow over both connection modes
//! ([`crate::mode::ConnectionMode`]):
//!
//! - **LAN**: a direct socket between peers (or mDNS discovery), no server.
//! - **Web**: a remote WebSocket relayed by `qcast-server`, with a STUN/TURN relay.
//!
//! The concrete `Signaling` transport (send/recv a [`SignalMessage`]) lives in
//! the sender/receiver crates where the LAN and Web backends are implemented.

use serde::{Deserialize, Serialize};

/// An identifier two peers agree on so they can find each other.
pub type RoomId = String;

/// Messages exchanged during WebRTC connection setup and teardown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    /// Join a room. `token` is required in Web mode (auth) and ignored on LAN.
    Join {
        room: RoomId,
        #[serde(default)]
        token: Option<String>,
    },
    /// SDP offer (from the sender/host).
    Offer { sdp: String },
    /// SDP answer (from the receiver).
    Answer { sdp: String },
    /// A trickled ICE candidate.
    IceCandidate {
        candidate: String,
        sdp_m_line_index: u32,
    },
    /// Peer is leaving / the session is closing.
    Bye,
}

impl SignalMessage {
    /// Serialize to a JSON string for transmission.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Parse from a received JSON string.
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_round_trips_through_json() {
        let msg = SignalMessage::Offer { sdp: "v=0\r\n".into() };
        let json = msg.to_json().unwrap();
        assert_eq!(SignalMessage::from_json(&json).unwrap(), msg);
    }

    #[test]
    fn join_is_tagged_snake_case() {
        let msg = SignalMessage::Join { room: "r1".into(), token: None };
        assert!(msg.to_json().unwrap().contains("\"type\":\"join\""));
    }

    #[test]
    fn ice_candidate_round_trips() {
        let msg = SignalMessage::IceCandidate {
            candidate: "candidate:1 1 udp ...".into(),
            sdp_m_line_index: 0,
        };
        let json = msg.to_json().unwrap();
        assert_eq!(SignalMessage::from_json(&json).unwrap(), msg);
    }
}
