//! Qcast host library.
//!
//! The capture → encode → `webrtcsink` pipeline, the in-process TURN relay, the
//! relocatable-bundle plugin path, and the receiver→sender remote-control input
//! injection — exposed as a library so BOTH the `qcast-sender` binary and the Tauri
//! app's `share` role drive the same, already-tested host logic (rather than
//! duplicating it). The binary (`main.rs`) is a thin CLI/GUI wrapper over this.

pub mod access_code;
pub mod bundle;
pub mod capture;
pub mod gui;
pub mod host;
pub mod input;
pub mod mdns;
pub mod preflight;
pub mod turn;

// End-to-end integration tests live in-crate (not a top-level `tests/` dir) so they
// can reach internals like `host::start` / `turn::ensure_running` directly.
#[cfg(test)]
mod tests_integration;
