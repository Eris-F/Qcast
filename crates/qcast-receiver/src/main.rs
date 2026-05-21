//! Qcast receiver: `webrtcbin` -> decode -> render into our own pipeline.
//!
//! Phase 0 smoke test: prints the platform-selected decoder. The real pipeline,
//! window, and frame handoff land in Phase 1.

use anyhow::{Context, Result};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gstreamer::init().context("failed to initialize GStreamer")?;

    let decoder = qcast_core::elements::pick_h264_decoder()
        .context("no H.264 decoder available")?;

    tracing::info!(%decoder, "qcast-receiver selected decoder (Phase 1 pipeline TODO)");
    println!("H.264 decoder: {decoder}");
    Ok(())
}
