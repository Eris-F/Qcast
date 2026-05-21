//! Shared building blocks for Qcast: the signaling protocol, connection modes,
//! platform-agnostic GStreamer element selection, and `webrtcbin` helpers.
//!
//! The media pipeline (capture → encode → transport → decode → render) is
//! identical across [`mode::ConnectionMode`]s; only signaling transport and
//! ICE configuration differ.

pub mod config;
pub mod elements;
pub mod mode;
pub mod signaling;
pub mod webrtc;
