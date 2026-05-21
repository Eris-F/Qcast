//! The Qcast host: builds and supervises the `webrtcsink` pipeline that captures
//! the desktop and serves it (encode-once / serve-many, per-consumer congestion
//! control + adaptive bitrate, codec negotiation, RTX/FEC) plus the signalling +
//! web servers webrtcsink runs itself. Used by both the GUI and `--headless`.

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::capture;

/// Our custom web client, embedded into the binary at compile time. Embedding makes
/// the binary self-contained so one file works for both the AppImage and the
/// Windows bundle (no source tree on the end-user machine). At startup we extract
/// it to a fresh per-process temp dir and point webrtcsink's web server at that.
static WEB_CLIENT: include_dir::Dir =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/web-client");

/// Dev-ergonomics override: if this env var is set and points to an existing dir,
/// the web client is served straight from there (no extraction), so the web client
/// can be edited live without recompiling.
const WEB_CLIENT_DIR_ENV: &str = "QCAST_WEB_CLIENT_DIR";

/// Holds the directory we serve the web client from for the pipeline's lifetime.
/// When we extracted the embedded client to a temp dir, its `Drop` best-effort
/// removes that dir; for the dev override (an existing user dir) it removes nothing.
struct WebClientDir {
    path: PathBuf,
    /// True only when we created `path` ourselves (a temp dir) and own its cleanup.
    owned_temp: bool,
}

impl WebClientDir {
    /// Resolve the directory to serve the web client from.
    ///
    /// - If `QCAST_WEB_CLIENT_DIR` points at an existing dir, serve that directly
    ///   (dev override; not cleaned up).
    /// - Otherwise extract the embedded client into a fresh per-process temp dir
    ///   (`qcast-web-<pid>`) which is removed on `Drop`.
    fn prepare() -> Result<Self> {
        if let Some(dir) = std::env::var_os(WEB_CLIENT_DIR_ENV) {
            let path = PathBuf::from(&dir);
            if path.is_dir() {
                tracing::info!(dir = %path.display(), "serving web client from {WEB_CLIENT_DIR_ENV}");
                return Ok(Self {
                    path,
                    owned_temp: false,
                });
            }
            tracing::warn!(
                dir = %path.display(),
                "{WEB_CLIENT_DIR_ENV} set but not an existing dir; falling back to embedded client"
            );
        }

        let path = std::env::temp_dir().join(format!("qcast-web-{}", std::process::id()));
        // Start from a clean slate in case a previous run with the same PID crashed.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)
            .with_context(|| format!("create web-client temp dir {}", path.display()))?;
        WEB_CLIENT
            .extract(&path)
            .with_context(|| format!("extract embedded web client to {}", path.display()))?;
        tracing::debug!(dir = %path.display(), "extracted embedded web client");
        Ok(Self {
            path,
            owned_temp: true,
        })
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for WebClientDir {
    fn drop(&mut self) {
        if self.owned_temp {
            // Best-effort cleanup; a leftover temp dir is harmless and self-healing
            // (the next run with this PID clears it before extracting).
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// Cap the captured video to a 1080p box before encoding (see [`build_pipeline`]).
const VIDEO_MAX_WIDTH: u32 = 1920;
const VIDEO_MAX_HEIGHT: u32 = 1080;

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
    /// The viewer access code (the "password" the operator shares). Generated
    /// once in `main()`; the single source of truth used by both the GUI display
    /// and the served `session.json`. NOTE: this is a client-side UX gate, not
    /// enforced authentication — anyone on the LAN can read `session.json`.
    pub access_code: String,
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

/// Write `session.json` into the served web-client directory. The browser fetches
/// this on load to learn the expected access code for the password gate. Kept
/// deliberately small: `{"name":"qcast","auth":"GHF/ABA/6TJ"}`.
fn write_session_json(dir: &std::path::Path, access_code: &str) -> Result<()> {
    let session = serde_json::json!({ "name": "qcast", "auth": access_code });
    let body = serde_json::to_string(&session).context("serialize session.json")?;
    let path = dir.join("session.json");
    std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    tracing::debug!(path = %path.display(), "wrote session.json");
    Ok(())
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

    // We force ICE relay, so a TURN relay must be up. Start (or reuse) it on this
    // thread's runtime before building the pipeline; it's torn down at the end.
    let turn_relay = match crate::turn::ensure_running(&rt, &lan_ip) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(Err(e.context("TURN relay")));
            return;
        }
    };

    // Extract (or locate) the web client. The guard lives until the end of this
    // function alongside the TURN relay / runtime teardown, so the served files
    // exist for the whole pipeline lifetime and the temp dir is removed on exit.
    let web_client = match WebClientDir::prepare() {
        Ok(w) => w,
        Err(e) => {
            let _ = tx.send(Err(e.context("prepare web client")));
            return;
        }
    };

    // Write session.json into the served directory BEFORE the pipeline starts so
    // it's available the instant the web server comes up. This carries the access
    // code to the browser gate. Works for both the extracted-temp-dir case and
    // the QCAST_WEB_CLIENT_DIR dev override (session.json is gitignored there).
    if let Err(e) = write_session_json(web_client.path(), &cfg.access_code) {
        let _ = tx.send(Err(e.context("write session.json")));
        return;
    }

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

    let pipeline = match build_pipeline(&source_desc, &cfg, &lan_ip, web_client.path()) {
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
    crate::turn::shutdown(&rt, turn_relay);
    // Drop the web-client guard explicitly after the pipeline is torn down so the
    // served temp dir outlives every consumer; its `Drop` removes the temp dir.
    drop(web_client);
    drop(rt);
}

/// Build the `… ! videoconvert ! videoscale ! webrtcsink` pipeline.
///
/// The `videoscale ! …,width=1920,height=1080` cap is required for decodability:
/// browser WebRTC decoders advertise hard frame-size ceilings (Firefox H.264 =
/// constrained-baseline Level 3.1 ≈ 720p; VP8 max-fs=12288 MB ≈ 3.1 MP). A capture
/// larger than 1080p exceeds those ceilings, so the decoder cannot decode the
/// frames. Capping to a 1080p box (`add-borders` letterboxes any aspect) keeps a
/// 1080p baseline while staying decodable everywhere; webrtcsink still adapts
/// bitrate/scale down per consumer.
fn build_pipeline(
    source_desc: &str,
    cfg: &HostConfig,
    lan_ip: &str,
    web_client_dir: &std::path::Path,
) -> Result<gst::Pipeline> {
    let desc = format!(
        "{source_desc} ! videoconvert ! videoscale add-borders=true \
         ! video/x-raw,width={vw},height={vh},pixel-aspect-ratio=1/1 ! webrtcsink name=ws \
         run-signalling-server=true signalling-server-host={host} signalling-server-port={sport} \
         run-web-server=true web-server-host-addr=http://{host}:{wport} web-server-directory={dir} \
         enable-control-data-channel=true",
        vw = VIDEO_MAX_WIDTH,
        vh = VIDEO_MAX_HEIGHT,
        host = cfg.host,
        sport = cfg.signalling_port,
        wport = cfg.web_port,
        dir = web_client_dir.display(),
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
        // TURN relay (the built-in in-process relay in `turn.rs`). The client also
        // forces relay, so ICE collapses to a single relay↔relay candidate pair.
        let turn_url = format!(
            "turn://{user}:{pass}@{lan_ip}:{port}",
            user = crate::turn::USER,
            pass = crate::turn::PASS,
            port = crate::turn::PORT,
        );
        ws.set_property("turn-servers", gst::Array::new([turn_url]));

        // Force ICE *relay* transport for reliability. Under the default "all"
        // policy, libnice gathers the full candidate matrix (host + srflx + mDNS +
        // ICE-TCP + relay across every interface, including Docker/VPN/link-local);
        // when many candidates are gathered, its nomination tick can hit a libnice
        // nomination assertion that aborts the process. Relay-only leaves exactly
        // one candidate pair per component, removing the nomination race. The
        // built-in TURN relay is always present, so a relay candidate is available.
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
