//! Qcast sender / host. **The app is the server**: it listens on a port and
//! receivers connect to it — directly on LAN, or via that same port exposed
//! over the web. This increment establishes the embedded signaling server;
//! WebRTC media negotiation (capture -> encode -> webrtcbin) is wired on top
//! in the next increment, inside [`handle_peer`].

use anyhow::{Context, Result};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use qcast_core::signaling::SignalMessage;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser, Debug)]
#[command(name = "qcast-sender", about = "Qcast host: serves a desktop stream on a port")]
struct Args {
    /// Connection mode (informs ICE config once media is wired): lan | web.
    #[arg(long, default_value = "lan")]
    mode: String,
    /// Address:port the host server listens on — the port you expose.
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: String,
    /// Room id that peers must agree on.
    #[arg(long, default_value = "qcast")]
    room: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gstreamer::init().context("failed to initialize GStreamer")?;

    let args = Args::parse();
    let sel = qcast_core::elements::probe();
    tracing::info!(source = ?sel.source, encoder = ?sel.encoder,
        "component-agnostic element selection");

    let listener = TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    tracing::info!(listen = %args.listen, mode = %args.mode, room = %args.room,
        "qcast-sender is serving — connect a receiver to ws://<host>:<port>");

    loop {
        let (stream, peer) = listener.accept().await.context("accept")?;
        let room = args.room.clone();
        tokio::spawn(async move {
            tracing::info!(%peer, "peer connected");
            if let Err(e) = handle_peer(stream, room).await {
                tracing::warn!(%peer, error = %e, "peer session ended");
            }
        });
    }
}

/// Handle one receiver connection.
///
/// TODO(next): on `Join`, build the webrtcbin offer (capture -> encode ->
/// webrtcbin), then exchange SDP/ICE with the peer over this socket.
async fn handle_peer(stream: TcpStream, room: String) -> Result<()> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .context("websocket handshake")?;
    let (mut tx, mut rx) = ws.split();

    while let Some(msg) = rx.next().await {
        match msg.context("ws read")? {
            Message::Text(txt) => match SignalMessage::from_json(txt.as_str()) {
                Ok(SignalMessage::Join { room: r, .. }) => {
                    if r != room {
                        tracing::warn!(expected = %room, got = %r, "room mismatch — rejecting");
                        tx.send(Message::text(SignalMessage::Bye.to_json()?)).await?;
                        break;
                    }
                    tracing::info!(room = %r, "receiver joined (WebRTC offer: TODO next increment)");
                    // Placeholder until media negotiation lands: acknowledge and close.
                    tx.send(Message::text(SignalMessage::Bye.to_json()?)).await?;
                    break;
                }
                Ok(other) => tracing::info!(?other, "signal"),
                Err(e) => tracing::warn!(error = %e, "invalid signal json"),
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(())
}
