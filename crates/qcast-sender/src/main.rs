//! Qcast host. **The app is the server.** It captures + encodes the desktop and
//! serves, on one `--listen` port:
//!   * `GET /`           the web client (a simple `<video>` viewer)
//!   * `GET /client.js`  the client script
//!   * `GET /ws`         WebSocket signaling (SDP/ICE) per browser
//!
//! Each browser that connects gets its own `webrtcbin` pipeline; the host
//! creates the WebRTC offer and the browser answers. No install on the
//! receiving end — any browser on any device.
//!
//! Local-first: capture is `videotestsrc` until the path is proven, then real
//! screen capture. The hosted/Web path (TURN, TLS) is deferred.

use anyhow::{anyhow, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_webrtc as gst_webrtc;
use qcast_core::signaling::SignalMessage;
use qcast_core::webrtc::session_description;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedSender};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

mod capture;

const INDEX_HTML: &str = include_str!("../web/index.html");
const CLIENT_JS: &str = include_str!("../web/client.js");

#[derive(Parser, Debug)]
#[command(name = "qcast-sender", about = "Qcast host: serves a desktop stream to browsers")]
struct Args {
    /// Connection mode (informs ICE config once the hosted path lands): lan | web.
    #[arg(long, default_value = "lan")]
    mode: String,
    /// Address:port to serve on — the port you open in a browser.
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: String,
    /// Capture source: `auto` (real screen via portal, falls back to test) or `test`.
    #[arg(long, default_value = "auto")]
    source: String,
}

/// Where the host gets video frames.
#[derive(Clone)]
enum SourceSpec {
    /// Built-in test pattern (no capture, no portal prompt).
    Test,
    /// Real desktop via xdg-desktop-portal + PipeWire.
    Screen { fd: i32, node_id: u32 },
}

#[derive(Clone)]
struct AppState {
    encoder: String,
    source: SourceSpec,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gst::init().context("failed to initialize GStreamer")?;

    let args = Args::parse();

    let missing = qcast_core::elements::missing_webrtc_support();
    if !missing.is_empty() {
        anyhow::bail!(
            "missing GStreamer plugins for WebRTC: {missing:?}. \
             On Fedora: `sudo dnf install -y libnice-gstreamer1 gstreamer1-plugins-bad-free`"
        );
    }

    let sel = qcast_core::elements::probe();
    let encoder = sel.encoder.context("no H.264 encoder available")?;
    tracing::info!(capture = ?sel.source, %encoder, mode = %args.mode,
        "component-agnostic element selection");

    let source = if args.source == "test" {
        tracing::info!("using test pattern (--source test)");
        SourceSpec::Test
    } else {
        match capture::acquire().await {
            Ok((fd, node_id)) => {
                tracing::info!(fd, node_id, "capturing desktop via xdg-desktop-portal");
                SourceSpec::Screen { fd, node_id }
            }
            Err(e) => {
                tracing::warn!(error = ?e, "portal capture unavailable; falling back to test pattern");
                SourceSpec::Test
            }
        }
    };

    let state = AppState { encoder, source };
    let app = Router::new()
        .route("/", get(index))
        .route("/client.js", get(client_js))
        .route("/favicon.ico", get(favicon))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let listener = TcpListener::bind(&args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    let port = args.listen.rsplit(':').next().unwrap_or("8080");
    match primary_lan_ip() {
        Some(ip) => tracing::info!(
            "qcast host serving — open  http://{ip}:{port}/  on any device (this machine: http://127.0.0.1:{port}/)"
        ),
        None => tracing::info!("qcast host serving — open http://<host>:{port}/ in any browser"),
    }
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

/// Best-effort primary LAN IP, so the startup log prints a URL reachable from
/// other devices (phone/tablet). Uses a connected UDP socket — no packets sent.
fn primary_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn client_js() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        CLIENT_JS,
    )
}

async fn favicon() -> axum::http::StatusCode {
    axum::http::StatusCode::NO_CONTENT
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| async move {
        tracing::info!("browser connected");
        if let Err(e) = run_session(socket, &state.encoder, &state.source).await {
            tracing::warn!(error = ?e, "session ended with error");
        }
        tracing::info!("session closed");
    })
}

/// One browser viewer: build a webrtcbin pipeline, offer, and pump SDP/ICE
/// between the gstreamer callbacks (via an mpsc channel) and this WebSocket.
async fn run_session(socket: WebSocket, encoder: &str, source: &SourceSpec) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (sig_tx, mut sig_rx) = mpsc::unbounded_channel::<SignalMessage>();

    let (pipeline, webrtcbin, frames) = build_pipeline(encoder, source, sig_tx)?;
    pipeline
        .set_state(gst::State::Playing)
        .context("set pipeline to Playing")?;

    // Stall detector: log frames/s leaving the encoder; warn loudly on zero.
    let fps_task = tokio::spawn({
        let frames = frames.clone();
        async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let mut last = 0u64;
            loop {
                interval.tick().await;
                let total = frames.load(Ordering::Relaxed);
                let fps = total - last;
                last = total;
                if fps == 0 {
                    tracing::warn!(total, "STALL: 0 frames left the encoder in the last second");
                } else {
                    tracing::info!(fps, total, "frames/s");
                }
            }
        }
    });

    loop {
        tokio::select! {
            outgoing = sig_rx.recv() => match outgoing {
                Some(msg) => {
                    if ws_tx.send(Message::Text(msg.to_json()?.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = ws_rx.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    if let Err(e) = apply_remote(&webrtcbin, text.as_str()) {
                        tracing::warn!(error = %e, "failed to apply remote signal");
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "websocket error");
                    break;
                }
                _ => {}
            },
        }
    }

    fps_task.abort();
    let _ = pipeline.set_state(gst::State::Null);
    Ok(())
}

/// Build `videotestsrc -> {encoder} -> rtph264pay -> webrtcbin` and wire the
/// offer + ICE callbacks to `sig_tx`.
fn build_pipeline(
    encoder: &str,
    source: &SourceSpec,
    sig_tx: UnboundedSender<SignalMessage>,
) -> Result<(gst::Pipeline, gst::Element, Arc<AtomicU64>)> {
    // Source branch produces NV12 raw video, scaled to a mobile-safe 720p
    // (letterboxed to preserve aspect) at a capped framerate. The shared tail
    // encodes H.264 as **constrained-baseline** — the profile every mobile
    // browser decoder handles — then RTP-payloads into webrtcbin.
    let source_desc = match source {
        SourceSpec::Test => "videotestsrc is-live=true pattern=ball ! \
             video/x-raw,width=1280,height=720,framerate=30/1 ! videoconvert"
            .to_string(),
        SourceSpec::Screen { fd, node_id } => format!(
            // do-timestamp + keepalive-time: KWin's capture is damage-driven and
            // stops emitting frames on a static screen; keepalive resends the last
            // buffer so the stream never goes dead. drop-only avoids retroactive
            // duplicate bursts after an idle gap.
            "pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=1000 ! \
             videoconvert ! videoscale add-borders=true ! \
             video/x-raw,format=NV12,width=1280,height=720 ! \
             videorate drop-only=true ! video/x-raw,framerate=30/1"
        ),
    };
    let desc = format!(
        "{source_desc} ! \
         {encoder} ! video/x-h264,profile=constrained-baseline ! h264parse ! \
         rtph264pay name=pay config-interval=-1 pt=96 ! \
         application/x-rtp,media=video,encoding-name=H264,payload=96 ! \
         webrtcbin name=qwebrtc bundle-policy=max-bundle"
    );
    let pipeline = gst::parse::launch(&desc)
        .context("parse host pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("parsed description is not a pipeline"))?;
    let webrtcbin = pipeline.by_name("qwebrtc").context("webrtcbin not found")?;

    // --- diagnostics ---------------------------------------------------------
    // Pipeline errors/warnings/EOS, logged synchronously (no glib main loop).
    if let Some(bus) = pipeline.bus() {
        bus.set_sync_handler(|_bus, msg| {
            use gst::MessageView;
            match msg.view() {
                MessageView::Error(e) => tracing::error!(
                    src = ?e.src().map(|s| s.path_string()),
                    error = %e.error(), debug = ?e.debug(), "gst ERROR"),
                MessageView::Warning(w) => tracing::warn!(
                    src = ?w.src().map(|s| s.path_string()),
                    error = %w.error(), "gst warning"),
                MessageView::Eos(_) => tracing::warn!("gst EOS (stream ended)"),
                _ => {}
            }
            gst::BusSyncReply::Pass
        });
    }

    // WebRTC connection-state transitions.
    webrtcbin.connect_notify(Some("connection-state"), |wb, _| {
        tracing::info!(state = ?wb.property_value("connection-state"), "webrtc connection-state");
    });
    webrtcbin.connect_notify(Some("ice-connection-state"), |wb, _| {
        tracing::info!(state = ?wb.property_value("ice-connection-state"), "webrtc ice-connection-state");
    });

    // Count buffers leaving the payloader to detect stalls (see frames/s logger).
    let frames = Arc::new(AtomicU64::new(0));
    match pipeline.by_name("pay").and_then(|p| p.static_pad("src")) {
        Some(srcpad) => {
            let frames = frames.clone();
            srcpad.add_probe(gst::PadProbeType::BUFFER, move |_, _| {
                frames.fetch_add(1, Ordering::Relaxed);
                gst::PadProbeReturn::Ok
            });
        }
        None => tracing::warn!("payloader src pad not found; frame-stall logging disabled"),
    }
    // -------------------------------------------------------------------------

    // on-negotiation-needed -> create-offer -> set-local-description -> send Offer.
    let tx = sig_tx.clone();
    webrtcbin.connect("on-negotiation-needed", false, move |values| {
        let webrtcbin = values[0].get::<gst::Element>().expect("webrtcbin element");
        let tx = tx.clone();
        let wb = webrtcbin.clone();
        let promise = gst::Promise::with_change_func(move |reply| {
            let reply = match reply {
                Ok(Some(reply)) => reply,
                _ => {
                    tracing::warn!("create-offer returned no reply");
                    return;
                }
            };
            let offer = match reply.get::<gst_webrtc::WebRTCSessionDescription>("offer") {
                Ok(offer) => offer,
                Err(e) => {
                    tracing::warn!(%e, "no offer in reply");
                    return;
                }
            };
            wb.emit_by_name::<()>("set-local-description", &[&offer, &None::<gst::Promise>]);
            match offer.sdp().as_text() {
                Ok(sdp) => {
                    let _ = tx.send(SignalMessage::Offer { sdp: sdp.to_string() });
                }
                Err(_) => tracing::warn!("could not serialize offer SDP"),
            }
        });
        webrtcbin.emit_by_name::<()>("create-offer", &[&None::<gst::Structure>, &promise]);
        None
    });

    // on-ice-candidate -> send IceCandidate.
    let tx = sig_tx;
    webrtcbin.connect("on-ice-candidate", false, move |values| {
        let mline = values[1].get::<u32>().expect("mline index");
        let candidate = values[2].get::<String>().expect("candidate");
        let _ = tx.send(SignalMessage::IceCandidate {
            candidate,
            sdp_m_line_index: mline,
        });
        None
    });

    Ok((pipeline, webrtcbin, frames))
}

/// Apply a signal received from the browser to the webrtcbin.
fn apply_remote(webrtcbin: &gst::Element, text: &str) -> Result<()> {
    match SignalMessage::from_json(text)? {
        SignalMessage::Answer { sdp } => {
            let answer = session_description(gst_webrtc::WebRTCSDPType::Answer, &sdp)?;
            webrtcbin.emit_by_name::<()>("set-remote-description", &[&answer, &None::<gst::Promise>]);
            tracing::info!("applied remote answer");
        }
        SignalMessage::IceCandidate { candidate, sdp_m_line_index } => {
            webrtcbin.emit_by_name::<()>("add-ice-candidate", &[&sdp_m_line_index, &candidate]);
        }
        other => tracing::debug!(?other, "ignoring signal"),
    }
    Ok(())
}
