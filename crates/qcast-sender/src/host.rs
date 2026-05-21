//! The Qcast host: builds and supervises the `webrtcsink` pipeline that captures
//! the desktop and serves it (encode-once / serve-many, per-consumer congestion
//! control + adaptive bitrate, codec negotiation, RTX/FEC) plus the signalling +
//! web servers webrtcsink runs itself. Used by both the GUI and `--headless`.

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::capture;

/// Our custom web client, served by webrtcsink's web server. Absolute path baked
/// in at compile time (bundled for distribution later).
pub const WEB_CLIENT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/web-client");

/// How the host should be configured. Cloneable so the GUI can keep a copy for
/// "retry" after a failed start.
#[derive(Clone)]
pub struct HostConfig {
    /// Bind address for the web + signalling servers (e.g. `0.0.0.0`).
    pub host: String,
    /// Port for the web client (browsers open this).
    pub web_port: u16,
    /// Port for the WebRTC signalling server.
    pub signalling_port: u16,
    /// Use the built-in test pattern instead of real screen capture.
    pub test_pattern: bool,
}

/// A live host. Dropping it (or calling [`RunningHost::stop`]) tears the pipeline
/// down and releases the capture.
pub struct RunningHost {
    quit: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// The viewer URL to share (e.g. `http://192.168.0.119:8080/`).
    pub url: String,
}

impl RunningHost {
    /// Signal the pipeline thread to stop and wait for it to finish.
    pub fn stop(&mut self) {
        self.quit.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Resolve the LAN IP and the viewer URL without starting anything (used by the
/// preflight screen, which shows the URL before the operator commits).
pub fn lan_url(host: &str, web_port: u16) -> (String, String) {
    let lan_ip = primary_lan_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| host.to_string());
    let url = format!("http://{lan_ip}:{web_port}/");
    (lan_ip, url)
}

/// Start the host on a dedicated thread. Blocks until the pipeline reaches
/// `Playing` (or fails), so the caller knows the URL is live before returning —
/// this includes waiting for the operator to approve the screen-share dialog.
pub fn start(cfg: HostConfig) -> Result<RunningHost> {
    let (lan_ip, url) = lan_url(&cfg.host, cfg.web_port);
    let quit = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<()>>();

    let q = quit.clone();
    let ip = lan_ip.clone();
    let handle = thread::Builder::new()
        .name("qcast-host".into())
        .spawn(move || run_pipeline(cfg, ip, q, tx))
        .context("spawn host thread")?;

    // Generous timeout: the portal picker is a human-in-the-loop step.
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(())) => Ok(RunningHost {
            quit,
            handle: Some(handle),
            url,
        }),
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(e)
        }
        Err(_) => {
            quit.store(true, Ordering::SeqCst);
            let _ = handle.join();
            Err(anyhow!("timed out waiting for capture/pipeline to start"))
        }
    }
}

/// Owns the pipeline + the tokio runtime for its whole lifetime, then runs a
/// simple bus-watch loop until asked to quit. Reports the startup result on `tx`.
fn run_pipeline(
    cfg: HostConfig,
    lan_ip: String,
    quit: Arc<AtomicBool>,
    tx: mpsc::Sender<Result<()>>,
) {
    // The xdg-desktop-portal capture needs an async (ashpd) runtime; it must stay
    // alive for the whole pipeline so the zbus connection / portal session holds.
    let rt = match tokio::runtime::Runtime::new().context("create tokio runtime") {
        Ok(rt) => rt,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };

    let source_desc = if cfg.test_pattern {
        tracing::info!("using test pattern");
        "videotestsrc is-live=true pattern=ball".to_string()
    } else {
        match capture::source_description(&rt) {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(Err(e.context("acquire capture source")));
                return;
            }
        }
    };

    let pipeline = match build_pipeline(&source_desc, &cfg, &lan_ip) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = pipeline
        .set_state(gst::State::Playing)
        .context("set pipeline to Playing")
    {
        let _ = tx.send(Err(e));
        let _ = pipeline.set_state(gst::State::Null);
        return;
    }
    let _ = tx.send(Ok(()));

    // Watch the bus until told to quit (or the pipeline errors / EOSes).
    let bus = pipeline.bus().expect("pipeline has a bus");
    while !quit.load(Ordering::SeqCst) {
        let Some(msg) = bus.timed_pop(Some(gst::ClockTime::from_mseconds(200))) else {
            continue;
        };
        use gst::MessageView;
        match msg.view() {
            MessageView::Error(e) => {
                tracing::error!(
                    src = ?e.src().map(|s| s.path_string()),
                    error = %e.error(), debug = ?e.debug(), "gst ERROR");
                break;
            }
            MessageView::Warning(w) => tracing::warn!(error = %w.error(), "gst warning"),
            MessageView::Eos(_) => {
                tracing::warn!("EOS");
                break;
            }
            _ => {}
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
    drop(rt);
}

/// Build the `… ! videoconvert ! videoscale ! webrtcsink` pipeline.
///
/// IMPORTANT — the `videoscale ! …,width=1920,height=1080` cap is load-bearing,
/// do NOT remove it to "send native resolution". Browser WebRTC decoders advertise
/// hard frame-size ceilings (Firefox H.264 = constrained-baseline Level 3.1 → 720p;
/// VP8 max-fs=12288 MB ≈ 3.1 MP). A 3440×1440 (4.95 MP) capture exceeds all of them,
/// so the decoder drops every frame → "connects but no video". Capping to a 1080p
/// box (add-borders letterboxes any aspect) keeps the 1080p baseline while staying
/// decodable everywhere; webrtcsink still adapts bitrate/scale down per consumer.
fn build_pipeline(source_desc: &str, cfg: &HostConfig, lan_ip: &str) -> Result<gst::Pipeline> {
    let desc = format!(
        "{source_desc} ! videoconvert ! videoscale add-borders=true \
         ! video/x-raw,width=1920,height=1080,pixel-aspect-ratio=1/1 ! webrtcsink name=ws \
         run-signalling-server=true signalling-server-host={host} signalling-server-port={sport} \
         run-web-server=true web-server-host-addr=http://{host}:{wport} web-server-directory={dir} \
         enable-control-data-channel=true",
        host = cfg.host,
        sport = cfg.signalling_port,
        wport = cfg.web_port,
        dir = WEB_CLIENT_DIR,
    );
    let pipeline = gst::parse::launch(&desc)
        .context("parse webrtcsink pipeline")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow!("parsed description is not a pipeline"))?;

    if let Some(ws) = pipeline.by_name("ws") {
        // Prefer VP8 over H.264. VP8 has no profile/level ceiling, so a 1080p frame
        // decodes on every browser (incl. Firefox, whose H.264 is locked to L3.1/720p).
        // H.264 stays available as a fallback for devices that advertise a high enough
        // level (Chrome/Safari/Android) and can hardware-decode it.
        if let Ok(caps) = gst::Caps::from_str("video/x-vp8;video/x-h264") {
            ws.set_property("video-caps", &caps);
        }
        // "extra data" #1: static per-stream metadata for our custom client.
        ws.set_property(
            "meta",
            gst::Structure::builder("meta").field("name", "qcast").build(),
        );
        // TURN relay (coturn on the LAN host). The client also forces relay, so
        // ICE collapses to a single relay↔relay candidate pair.
        let turn_url = format!("turn://qcast:qcastpass@{lan_ip}:3478");
        ws.set_property("turn-servers", gst::Array::new([turn_url]));

        // Force ICE *relay* transport. This is load-bearing for reliability, not
        // an optimization: with the default "all" policy, libnice gathers the full
        // candidate matrix (host + srflx + mDNS + ICE-TCP + relay across every
        // interface — Docker/VPN/link-local included), and on a real remote device
        // its nomination tick hits an assertion (conncheck.c: NICE_CHECK_SUCCEEDED)
        // that ABORTS the whole host process. Relay-only leaves exactly one pair
        // per component, so there's no nomination race to crash on. Requires coturn.
        ws.set_property_from_str("ice-transport-policy", "relay");
    }

    Ok(pipeline)
}

/// Best-effort primary LAN IP (via a connected UDP socket — no packets sent), so
/// the viewer URL is reachable from other devices.
fn primary_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    sock.local_addr().ok().map(|addr| addr.ip())
}
