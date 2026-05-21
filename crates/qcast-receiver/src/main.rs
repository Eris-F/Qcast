//! Qcast receiver / viewer. Connects to the host's exposed port and (next
//! increment) receives WebRTC media to decode and render into our own pipeline.

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use qcast_core::signaling::SignalMessage;
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser, Debug)]
#[command(name = "qcast-receiver", about = "Qcast viewer: connects to a host and renders its stream")]
struct Args {
    /// WebSocket URL of the host server (the exposed port).
    #[arg(long, default_value = "ws://127.0.0.1:8080")]
    connect: String,
    /// Room id that must match the host.
    #[arg(long, default_value = "qcast")]
    room: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gstreamer::init().context("failed to initialize GStreamer")?;
    let args = Args::parse();

    let decoder = qcast_core::elements::pick_h264_decoder();
    tracing::info!(?decoder, "component-agnostic decoder selection");

    let (ws, _resp) = tokio_tungstenite::connect_async(&args.connect)
        .await
        .with_context(|| format!("connecting to {}", args.connect))?;
    tracing::info!(url = %args.connect, "connected to host");
    let (mut tx, mut rx) = ws.split();

    tx.send(Message::text(
        SignalMessage::Join { room: args.room.clone(), token: None }.to_json()?,
    ))
    .await?;
    tracing::info!(room = %args.room, "sent join");

    // TODO(next): create a webrtcbin answerer, set-remote-description from the
    // host's offer, send the answer, exchange ICE, decode -> render.
    while let Some(msg) = rx.next().await {
        match msg.context("ws read")? {
            Message::Text(txt) => match SignalMessage::from_json(txt.as_str()) {
                Ok(m) => {
                    tracing::info!(?m, "signal");
                    if matches!(m, SignalMessage::Bye) {
                        break;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "invalid signal json"),
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    tracing::info!("receiver done");
    Ok(())
}
