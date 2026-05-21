//! Qcast sender: capture -> encode -> `webrtcbin`.
//!
//! Phase 0 smoke test: prints the platform-selected capture source and encoder
//! so we can confirm the component-agnostic selection on each machine. The real
//! pipeline + signaling land in Phase 1.

use anyhow::{Context, Result};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gstreamer::init().context("failed to initialize GStreamer")?;

    let sel = qcast_core::elements::probe();
    let source = sel.source.context("no screen-capture source available")?;
    let encoder = sel.encoder.context("no H.264 encoder available")?;

    tracing::info!(%source, %encoder, "qcast-sender selected elements (Phase 1 pipeline TODO)");
    println!("capture source: {source}");
    println!("H.264 encoder:  {encoder}");
    Ok(())
}
