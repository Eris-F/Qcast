//! Qcast host. Captures the desktop and serves it to browsers via gst-plugins-rs
//! `webrtcsink`, which encodes once and serves many consumers with per-consumer
//! congestion control + adaptive bitrate, codec negotiation, and loss recovery
//! (RTX/FEC) — and runs the signalling + web servers itself. We just build the
//! pipeline, propose codecs, and serve our custom web client.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use std::str::FromStr;

mod capture;

/// Our custom web client, served by webrtcsink's web server. Absolute path baked
/// at compile time (bundled for distribution later).
const WEB_CLIENT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/web-client");

#[derive(Parser, Debug)]
#[command(name = "qcast-sender", about = "Qcast host: serves a desktop stream to browsers")]
struct Args {
    /// Connection mode (LAN now; Web/TURN deferred).
    #[arg(long, default_value = "lan")]
    mode: String,
    /// Address to bind the web + signalling servers on.
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    /// Port for the web client (open this in a browser).
    #[arg(long, default_value_t = 8080)]
    web_port: u16,
    /// Port for the WebRTC signalling server.
    #[arg(long, default_value_t = 8443)]
    signalling_port: u16,
    /// Capture source: `auto` (real screen via portal, falls back to test) or `test`.
    #[arg(long, default_value = "auto")]
    source: String,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    gst::init().context("failed to initialize GStreamer")?;
    let args = Args::parse();

    if gst::ElementFactory::find("webrtcsink").is_none() {
        anyhow::bail!(
            "webrtcsink not found — build & install the gst-plugins-rs webrtc plugin \
             (see deploy/setup-linux.sh)"
        );
    }

    // The xdg-desktop-portal capture needs an async (ashpd) runtime; keep it alive
    // for the whole process so the zbus connection / portal session stays open.
    let rt = tokio::runtime::Runtime::new().context("tokio runtime")?;
    let source_desc = if args.source == "test" {
        tracing::info!("using test pattern (--source test)");
        "videotestsrc is-live=true pattern=ball".to_string()
    } else {
        match rt.block_on(capture::acquire()) {
            Ok((fd, node_id)) => {
                tracing::info!(fd, node_id, "capturing desktop via xdg-desktop-portal");
                format!("pipewiresrc fd={fd} path={node_id}")
            }
            Err(e) => {
                tracing::warn!(error = ?e, "portal capture unavailable; using test pattern");
                "videotestsrc is-live=true pattern=ball".to_string()
            }
        }
    };

    // webrtcsink runs the signalling + web servers and handles encode/adapt/recover.
    let desc = format!(
        "{source_desc} ! videoconvert ! webrtcsink name=ws \
         run-signalling-server=true signalling-server-host={host} signalling-server-port={sport} \
         run-web-server=true web-server-host-addr=http://{host}:{wport} web-server-directory={dir} \
         enable-control-data-channel=true",
        host = args.host,
        sport = args.signalling_port,
        wport = args.web_port,
        dir = WEB_CLIENT_DIR,
    );
    let pipeline = gst::parse::launch(&desc)
        .context("parse webrtcsink pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("parsed description is not a pipeline"))?;

    // Propose multiple codecs — each browser negotiates the best it supports
    // (H.264 for hardware decode on mobile, VP8 universally). Attach stream meta.
    if let Some(ws) = pipeline.by_name("ws") {
        if let Ok(caps) = gst::Caps::from_str("video/x-h264;video/x-vp8") {
            ws.set_property("video-caps", &caps);
        }
        ws.set_property("meta", gst::Structure::builder("meta").field("name", "qcast").build());
    }

    pipeline.set_state(gst::State::Playing).context("set pipeline to Playing")?;
    tracing::info!(
        "qcast host serving — open  http://{}:{}/  on any device",
        primary_lan_ip().map(|ip| ip.to_string()).unwrap_or_else(|| args.host.clone()),
        args.web_port,
    );

    // Run a glib main loop (keeps webrtcsink + its servers alive); log bus errors.
    let main_loop = glib::MainLoop::new(None, false);
    if let Some(bus) = pipeline.bus() {
        let ml = main_loop.clone();
        let _watch = bus
            .add_watch(move |_bus, msg| {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Error(e) => {
                        tracing::error!(
                            src = ?e.src().map(|s| s.path_string()),
                            error = %e.error(), debug = ?e.debug(), "gst ERROR");
                        ml.quit();
                    }
                    MessageView::Warning(w) => {
                        tracing::warn!(error = %w.error(), "gst warning")
                    }
                    MessageView::Eos(_) => {
                        tracing::warn!("EOS");
                        ml.quit();
                    }
                    _ => {}
                }
                glib::ControlFlow::Continue
            })
            .context("add bus watch")?;
        main_loop.run();
    }

    pipeline.set_state(gst::State::Null).ok();
    let _ = rt; // keep the portal runtime alive until shutdown
    Ok(())
}

/// Best-effort primary LAN IP so the log prints a URL reachable from other devices.
fn primary_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}
