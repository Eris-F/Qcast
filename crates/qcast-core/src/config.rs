//! Startup configuration, validated at the boundary before use.

use serde::{Deserialize, Serialize};

use crate::mode::{ConnectionMode, IceServer};

/// Application configuration shared by sender and receiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// LAN (direct) or Web (via the private server). Both are always available.
    pub mode: ConnectionMode,
    /// Room/stream id both peers agree on.
    pub room: String,
    /// Starting video bitrate in kbps. Congestion control adjusts this at
    /// runtime (Phase 3) so the connection survives a degrading link.
    pub bitrate_kbps: u32,
    /// Web mode only: signaling server WebSocket URL.
    #[serde(default)]
    pub signaling_url: Option<String>,
    /// Web mode only: STUN/TURN servers.
    #[serde(default)]
    pub ice_servers: Vec<IceServer>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ConnectionMode::Lan,
            room: "qcast".to_string(),
            bitrate_kbps: 4000,
            signaling_url: None,
            ice_servers: Vec::new(),
        }
    }
}

impl Config {
    /// Validate cross-field invariants at the boundary. Fail fast with a clear message.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.room.trim().is_empty() {
            anyhow::bail!("room id must not be empty");
        }
        if self.bitrate_kbps == 0 {
            anyhow::bail!("bitrate_kbps must be greater than 0");
        }
        if self.mode == ConnectionMode::Web && self.signaling_url.is_none() {
            anyhow::bail!("web mode requires a signaling_url");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_default_is_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn web_mode_requires_signaling_url() {
        let cfg = Config { mode: ConnectionMode::Web, ..Config::default() };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn zero_bitrate_is_rejected() {
        let cfg = Config { bitrate_kbps: 0, ..Config::default() };
        assert!(cfg.validate().is_err());
    }
}
