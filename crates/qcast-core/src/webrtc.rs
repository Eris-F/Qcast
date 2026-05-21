//! Shared `webrtcbin` setup used by both the sender and the receiver: building
//! the bin, applying [`crate::mode::IceConfig`], wiring the ICE-candidate and
//! `on-negotiation-needed` callbacks, and (Phase 3) attaching congestion
//! control (`rtpgccbwe`) that drives the encoder bitrate so the connection
//! survives a degrading link.
//!
//! TODO(Phase 1): port from the `gstreamer-rs` `webrtc` example.
