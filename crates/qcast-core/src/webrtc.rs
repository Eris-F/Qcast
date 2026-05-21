//! Shared `webrtcbin` helpers: SDP <-> `WebRTCSessionDescription` conversion and
//! ICE configuration from a connection mode. The negotiation orchestration is
//! tied to each app's signaling task and lives in the binaries.

use anyhow::{anyhow, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_sdp as gst_sdp;
use gstreamer_webrtc as gst_webrtc;

use crate::mode::IceConfig;

/// Build a `WebRTCSessionDescription` of `kind` from an SDP string.
pub fn session_description(
    kind: gst_webrtc::WebRTCSDPType,
    sdp: &str,
) -> Result<gst_webrtc::WebRTCSessionDescription> {
    let msg = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
        .map_err(|_| anyhow!("failed to parse SDP"))?;
    Ok(gst_webrtc::WebRTCSessionDescription::new(kind, msg))
}

/// Apply ICE servers (Web mode) to a webrtcbin. LAN mode passes an empty config
/// (host candidates only — no STUN/TURN).
pub fn configure_ice(webrtcbin: &gst::Element, ice: &IceConfig) {
    for server in &ice.servers {
        if server.uri.starts_with("stun") {
            webrtcbin.set_property("stun-server", server.uri.as_str());
        } else if server.uri.starts_with("turn") {
            let ok: bool = webrtcbin.emit_by_name("add-turn-server", &[&server.uri]);
            if !ok {
                tracing::warn!(uri = %server.uri, "webrtcbin rejected turn server");
            }
        }
    }
}
