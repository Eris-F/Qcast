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
use std::time::{Duration, Instant};

use crate::capture;

/// Max number of automatic pipeline restarts before the supervisor gives up and
/// shuts the host down cleanly (so the process never lingers pretending to stream).
const MAX_RESTART_ATTEMPTS: u32 = 5;
/// Backoff base: attempt N (1-indexed) waits `BACKOFF_BASE * 2^(N-1)`, capped at
/// [`BACKOFF_CAP`]. With a 1s base: 1s, 2s, 4s, 8s, 16s → capped to 8s.
const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Upper bound on a single backoff wait.
const BACKOFF_CAP: Duration = Duration::from_secs(8);
/// A session that ran at least this long is considered "healthy"; ending after
/// this resets the restart-attempt budget so isolated failures don't exhaust it.
const HEALTHY_RUN: Duration = Duration::from_secs(30);

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

/// Default cap for the captured video: a 1080p box before encoding (see
/// [`build_pipeline`]). This is the universally-decodable baseline; the operator
/// can override it via [`HostConfig::max_width`] / [`HostConfig::max_height`].
pub const VIDEO_MAX_WIDTH: u32 = 1920;
pub const VIDEO_MAX_HEIGHT: u32 = 1080;

/// Hard upper bound we accept for a custom resolution edge length. Above this we
/// reject at the boundary: webrtcsink scales per-consumer anyway, and arbitrarily
/// huge frames just waste encode/scale cycles while exceeding every browser
/// decoder's frame-size ceiling. 7680 covers an 8K edge (the practical maximum).
pub const RESOLUTION_MAX_EDGE: u32 = 7680;

/// Codec preference: governs the `video-caps` proposed to webrtcsink, both WHICH
/// codecs are offered and in what ORDER (the first listed is preferred during
/// negotiation).
///
/// Rationale: VP8 has no profile/level frame-size ceiling, so a 1080p frame
/// decodes on every browser (incl. Firefox, whose H.264 is locked to constrained-
/// baseline Level 3.1 ≈ 720p). H.264 hardware-decodes well on Chrome/Safari/
/// Android. Preferring VP8 is the safe default; the operator can flip the
/// preference or restrict to a single codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecPref {
    /// Offer both, VP8 preferred (default). Maps to `"video/x-vp8;video/x-h264"`.
    Auto,
    /// Offer both, H.264 preferred. Maps to `"video/x-h264;video/x-vp8"`.
    H264Preferred,
    /// Offer VP8 only. Maps to `"video/x-vp8"`.
    Vp8Only,
    /// Offer H.264 only. Maps to `"video/x-h264"`.
    H264Only,
}

impl CodecPref {
    /// The `video-caps` string for this preference (codec list + ordering). The
    /// first entry is the preferred codec during negotiation.
    pub fn video_caps(self) -> &'static str {
        match self {
            CodecPref::Auto => "video/x-vp8;video/x-h264",
            CodecPref::H264Preferred => "video/x-h264;video/x-vp8",
            CodecPref::Vp8Only => "video/x-vp8",
            CodecPref::H264Only => "video/x-h264",
        }
    }
}

impl Default for CodecPref {
    fn default() -> Self {
        CodecPref::Auto
    }
}

/// Validate an operator-chosen resolution cap at the boundary, before it reaches
/// the pipeline. Frame dimensions must be positive, even (most encoders/scalers
/// require even width+height for chroma subsampling), and within a sane edge
/// bound. Returns a clear, user-facing message on rejection.
pub fn validate_resolution(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        anyhow::bail!("resolution width and height must be greater than 0");
    }
    if width % 2 != 0 || height % 2 != 0 {
        anyhow::bail!(
            "resolution width and height must be even numbers (got {width}x{height})"
        );
    }
    if width > RESOLUTION_MAX_EDGE || height > RESOLUTION_MAX_EDGE {
        anyhow::bail!(
            "resolution {width}x{height} exceeds the maximum supported edge of \
             {RESOLUTION_MAX_EDGE}px"
        );
    }
    Ok(())
}

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
    /// Max captured-frame width before encoding (the `videoscale` cap). Defaults
    /// to [`VIDEO_MAX_WIDTH`] (1080p baseline). Values above 1080p may not decode
    /// on every browser.
    pub max_width: u32,
    /// Max captured-frame height before encoding (the `videoscale` cap). Defaults
    /// to [`VIDEO_MAX_HEIGHT`] (1080p baseline).
    pub max_height: u32,
    /// Which video codec(s) to propose to viewers and in what preference order.
    pub codec_pref: CodecPref,
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

/// Why a single pipeline session stopped — drives the supervisor's restart logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndReason {
    /// The shared quit flag was set (user/operator asked to stop). Never restarts.
    Quit,
    /// The very first session never reached `Playing` (build or start failure). The
    /// caller is still waiting on `start`'s result, so we report the error and stop
    /// rather than retrying a startup that never worked — restart/backoff is for
    /// runtime failures of an already-live stream, not a never-started one.
    StartupFailed,
    /// The pipeline posted a GStreamer `Error` message. Eligible for restart.
    Error,
    /// The pipeline reached end-of-stream. Eligible for restart.
    Eos,
}

/// What the supervisor should do after a session ends — the pure, testable
/// decision separated from all the GStreamer / threading side effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestartDecision {
    /// Stop the host. Carries whether this was a clean user quit (vs. giving up
    /// after exhausting the restart budget), so the supervisor can log accordingly.
    Stop { user_quit: bool },
    /// Restart the pipeline as attempt `attempt` (1-indexed) after waiting `delay`.
    Restart { attempt: u32, delay: Duration },
}

/// Pure restart policy: given how many restart attempts have already been spent,
/// how long the session that just ended actually ran, and why it ended, decide
/// whether to restart (and after how long) or to stop.
///
/// Rules:
/// - A user quit always stops, never restarts.
/// - A first-session startup failure stops immediately (the caller already has
///   the error; retrying a startup that never worked just makes `start` hang
///   through the backoff schedule).
/// - A session that ran at least [`HEALTHY_RUN`] is treated as healthy: the
///   attempt budget is considered reset, so the next restart is attempt 1.
/// - Otherwise the failure counts against the budget; once [`MAX_RESTART_ATTEMPTS`]
///   have been spent we give up and stop.
/// - Backoff for attempt N is `BACKOFF_BASE * 2^(N-1)`, capped at [`BACKOFF_CAP`].
fn decide_restart(attempts_used: u32, session_ran: Duration, reason: EndReason) -> RestartDecision {
    if reason == EndReason::Quit {
        return RestartDecision::Stop { user_quit: true };
    }
    // A startup that never reached Playing isn't a recoverable runtime failure;
    // the caller already received the error, so stop without burning the budget.
    if reason == EndReason::StartupFailed {
        return RestartDecision::Stop { user_quit: false };
    }

    // A healthy run earns a clean slate: don't let occasional failures, spread
    // out over a long-running host, slowly drain the budget.
    let effective_used = if session_ran >= HEALTHY_RUN {
        0
    } else {
        attempts_used
    };

    if effective_used >= MAX_RESTART_ATTEMPTS {
        return RestartDecision::Stop { user_quit: false };
    }

    let attempt = effective_used + 1; // 1-indexed for logging + backoff math.
    let shift = (attempt - 1).min(16); // guard the shift against overflow.
    let delay = BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP);
    RestartDecision::Restart { attempt, delay }
}

/// Resources that live for the whole host lifetime — i.e. ACROSS pipeline
/// restarts — not for a single session. Rebuilding any of these on every restart
/// would be wrong: re-acquiring `source_desc` on Linux would re-trigger the
/// xdg-desktop-portal picker dialog (see the `source_desc` field), and re-binding
/// TURN / re-extracting the web client is pointless churn.
struct SupervisorResources {
    /// tokio runtime for the async portal/TURN work; must outlive every session.
    rt: tokio::runtime::Runtime,
    /// The TURN relay (or a handle to the reused external one). Stays up across
    /// restarts; only stopped on final shutdown. `Option` so it can be moved out
    /// for `turn::shutdown` at teardown.
    turn_relay: Option<crate::turn::Relay>,
    /// The served web-client directory guard. Its `Drop` removes the temp dir, so
    /// it must outlive every consumer of every session.
    web_client: WebClientDir,
    /// The capture source sub-pipeline string, acquired ONCE. On Linux real
    /// capture this came from the portal handshake (which shows the human picker);
    /// reusing the string avoids re-popping that picker on every restart.
    ///
    /// KNOWN LIMITATION (v1): the string embeds the PipeWire fd/node id from the
    /// first handshake. If that fd goes stale, restarts will fail immediately and
    /// repeatedly — but the bounded attempt budget + clean shutdown cap the damage.
    /// Re-acquiring a fresh fd without re-prompting the picker is future work.
    source_desc: String,
}

/// Owns the long-lived host resources, then supervises one-pipeline-session-at-a-
/// time, restarting on failure with bounded attempts + backoff. Reports the FIRST
/// session's startup result on `tx` so `start` can return success/failure; later
/// restarts are reported via logs only. On give-up it flips `quit` so the rest of
/// the process (run_background / run_headless) exits instead of lingering as if it
/// were still streaming.
fn run_pipeline(
    cfg: HostConfig,
    lan_ip: String,
    quit: Arc<AtomicBool>,
    tx: mpsc::Sender<Result<()>>,
) {
    let mut res = match prepare_resources(&cfg, &lan_ip) {
        Ok(res) => res,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };

    let mut attempts_used: u32 = 0;
    // The very first session reports startup on `tx`; restarts report via logs.
    let mut startup_tx = Some(tx);

    loop {
        if quit.load(Ordering::SeqCst) {
            break;
        }

        let session_start = Instant::now();
        let reason = run_one_session(&cfg, &lan_ip, &res, &quit, &mut startup_tx);
        let ran = session_start.elapsed();

        match decide_restart(attempts_used, ran, reason) {
            RestartDecision::Stop { user_quit } => {
                if user_quit {
                    tracing::info!("stream stopped (user quit)");
                } else if reason == EndReason::StartupFailed {
                    // The caller already received the error via `tx`; just stop.
                    // Flip quit so any other process loop also exits.
                    tracing::error!("stream failed to start; stopping");
                    quit.store(true, Ordering::SeqCst);
                } else {
                    tracing::error!(
                        attempts = MAX_RESTART_ATTEMPTS,
                        "stream could not be recovered after {MAX_RESTART_ATTEMPTS} attempts; \
                         stopping"
                    );
                    // Tell the rest of the process to exit so it doesn't keep
                    // running as if it were still streaming.
                    quit.store(true, Ordering::SeqCst);
                }
                break;
            }
            RestartDecision::Restart { attempt, delay } => {
                // A healthy run resets the budget (the decision already treated it
                // as attempt 1); reflect that in the counter we carry forward.
                attempts_used = attempt;
                tracing::warn!(
                    reason = ?reason,
                    attempt,
                    max = MAX_RESTART_ATTEMPTS,
                    delay_s = delay.as_secs(),
                    "stream stopped ({reason:?}); restarting (attempt {attempt}/{MAX_RESTART_ATTEMPTS}) \
                     in {}s",
                    delay.as_secs()
                );
                // Sleep in small slices so a quit during backoff is honored fast.
                if !sleep_interruptible(delay, &quit) {
                    break;
                }
            }
        }
    }

    // Final teardown of the long-lived resources (once, after the loop).
    if let Some(relay) = res.turn_relay.take() {
        crate::turn::shutdown(&res.rt, relay);
    }
    // Drop order: web_client guard removes the served temp dir, then the runtime.
    let SupervisorResources { rt, web_client, .. } = res;
    drop(web_client);
    drop(rt);
}

/// Acquire everything that must persist across restarts. The capture source
/// description (and thus, on Linux, the portal picker) is acquired here EXACTLY
/// ONCE.
fn prepare_resources(cfg: &HostConfig, lan_ip: &str) -> Result<SupervisorResources> {
    // The xdg-desktop-portal capture needs an async (ashpd) runtime; it must stay
    // alive for the whole host so the zbus connection / portal session holds.
    let rt = tokio::runtime::Runtime::new().context("create tokio runtime")?;

    // We force ICE relay, so a TURN relay must be up. Start (or reuse) it once.
    let turn_relay = crate::turn::ensure_running(&rt, lan_ip).context("TURN relay")?;

    // Extract (or locate) the web client once; the guard lives for the whole host.
    let web_client = WebClientDir::prepare().context("prepare web client")?;

    // Write session.json into the served directory BEFORE the pipeline starts so
    // it's available the instant the web server comes up. This carries the access
    // code to the browser gate. Works for both the extracted-temp-dir case and
    // the QCAST_WEB_CLIENT_DIR dev override (session.json is gitignored there).
    write_session_json(web_client.path(), &cfg.access_code).context("write session.json")?;

    // Acquire the capture source ONCE and reuse it across restarts. For real
    // Linux capture this runs the portal handshake (human picker); doing it here
    // — rather than per session — guarantees the picker can't pop on a restart.
    let source_desc = if cfg.test_pattern {
        tracing::info!("using test pattern");
        "videotestsrc is-live=true pattern=ball".to_string()
    } else {
        capture::source_description(&rt).context("acquire capture source")?
    };

    Ok(SupervisorResources {
        rt,
        turn_relay: Some(turn_relay),
        web_client,
        source_desc,
    })
}

/// Build + run ONE pipeline session, watching its bus until it ends, and report
/// WHY it ended. Uses the already-acquired long-lived resources (no portal
/// re-prompt, no TURN rebind). If `startup_tx` still holds a sender, the FIRST
/// start (success or failure) is reported through it and the sender is consumed.
fn run_one_session(
    cfg: &HostConfig,
    lan_ip: &str,
    res: &SupervisorResources,
    quit: &Arc<AtomicBool>,
    startup_tx: &mut Option<mpsc::Sender<Result<()>>>,
) -> EndReason {
    let pipeline = match build_pipeline(&res.source_desc, cfg, lan_ip, res.web_client.path()) {
        Ok(p) => p,
        Err(e) => {
            // Report a build failure only on first start (so `start` can surface
            // it) and stop — a never-started pipeline isn't worth retrying. On a
            // restart there's no one waiting, so log it and let the supervisor's
            // backoff/budget handle the Error end-reason.
            if let Some(tx) = startup_tx.take() {
                let _ = tx.send(Err(e));
                return EndReason::StartupFailed;
            }
            tracing::error!(error = %e, "failed to rebuild pipeline on restart");
            return EndReason::Error;
        }
    };

    if let Err(e) = pipeline
        .set_state(gst::State::Playing)
        .context("set pipeline to Playing")
    {
        let _ = pipeline.set_state(gst::State::Null);
        if let Some(tx) = startup_tx.take() {
            let _ = tx.send(Err(e));
            return EndReason::StartupFailed;
        }
        tracing::error!(error = %e, "failed to start pipeline on restart");
        return EndReason::Error;
    }
    // First successful start: let `start` return Ok and hand back the URL.
    if let Some(tx) = startup_tx.take() {
        let _ = tx.send(Ok(()));
    }

    // Watch the bus until told to quit (or the pipeline errors / EOSes).
    let bus = pipeline.bus().expect("pipeline has a bus");
    let mut reason = EndReason::Quit;
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
                reason = EndReason::Error;
                break;
            }
            MessageView::Warning(w) => tracing::warn!(error = %w.error(), "gst warning"),
            MessageView::Eos(_) => {
                tracing::warn!("EOS");
                reason = EndReason::Eos;
                break;
            }
            _ => {}
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
    reason
}

/// Sleep for `dur`, but wake early if `quit` is set. Returns `false` if a quit
/// was observed (caller should stop), `true` if the full sleep elapsed.
fn sleep_interruptible(dur: Duration, quit: &Arc<AtomicBool>) -> bool {
    let deadline = Instant::now() + dur;
    loop {
        if quit.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep(Duration::from_millis(100).min(deadline - now));
    }
}

/// Build the `… ! videoconvert ! videoscale ! webrtcsink` pipeline.
///
/// The `videoscale ! …,width=W,height=H` cap is required for decodability: browser
/// WebRTC decoders advertise hard frame-size ceilings (Firefox H.264 = constrained-
/// baseline Level 3.1 ≈ 720p; VP8 max-fs=12288 MB ≈ 3.1 MP). A capture larger than
/// 1080p exceeds those ceilings, so some browsers cannot decode the frames. The cap
/// defaults to a 1080p box ([`VIDEO_MAX_WIDTH`]×[`VIDEO_MAX_HEIGHT`]) — the
/// universally-decodable baseline — but the operator can override `cfg.max_width`/
/// `cfg.max_height`. `add-borders` letterboxes any aspect; webrtcsink still adapts
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
         enable-control-data-channel=true enable-data-channel-navigation=true",
        vw = cfg.max_width,
        vh = cfg.max_height,
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
        // Codec preference governs which codecs are proposed and in what order.
        // VP8 has no profile/level ceiling, so a 1080p frame decodes on every
        // browser (incl. Firefox, whose H.264 is locked to L3.1/720p); H.264 is a
        // hardware-friendly fallback on Chrome/Safari/Android. The default (Auto)
        // offers both with VP8 first; the operator may flip the order or restrict
        // to a single codec.
        if let Ok(caps) = gst::Caps::from_str(cfg.codec_pref.video_caps()) {
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

        // Receiver→sender remote control: webrtcsink surfaces the navigation events
        // the receiver sends over the data channel (enable-data-channel-navigation,
        // above) as upstream GstNavigation events. Probe them, decode to InputEvent,
        // and inject on this machine. `frame` is the negotiated capped frame
        // (cfg.max_width × max_height) used to normalize pointer coordinates.
        let injector = crate::input::shared_injector();
        let frame = (cfg.max_width as f64, cfg.max_height as f64);
        let probed = crate::input::attach_navigation_probes(&ws, frame, injector);
        tracing::debug!(pads = probed, "input: attached navigation probes to webrtcsink");
        if probed == 0 {
            tracing::warn!(
                "input: webrtcsink exposed no sink pad to probe — remote control will be inert"
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each codec preference maps to the expected `video-caps` string, with the
    /// preferred codec first (negotiation prefers the leading entry).
    #[test]
    fn codec_pref_maps_to_video_caps() {
        assert_eq!(CodecPref::Auto.video_caps(), "video/x-vp8;video/x-h264");
        assert_eq!(
            CodecPref::H264Preferred.video_caps(),
            "video/x-h264;video/x-vp8"
        );
        assert_eq!(CodecPref::Vp8Only.video_caps(), "video/x-vp8");
        assert_eq!(CodecPref::H264Only.video_caps(), "video/x-h264");
        // The default preference is VP8-preferred (Auto).
        assert_eq!(CodecPref::default(), CodecPref::Auto);
    }

    /// Each codec-preference video-caps string must be parseable by GStreamer
    /// (guards against typos in the mapping that would silently no-op the
    /// `video-caps` set in `build_pipeline`).
    #[test]
    fn codec_pref_video_caps_parse() {
        // `Caps::from_str` requires an initialized GStreamer; `init` is idempotent
        // and safe to call from a unit test.
        let _ = gst::init();
        for pref in [
            CodecPref::Auto,
            CodecPref::H264Preferred,
            CodecPref::Vp8Only,
            CodecPref::H264Only,
        ] {
            assert!(
                gst::Caps::from_str(pref.video_caps()).is_ok(),
                "video-caps for {pref:?} should parse"
            );
        }
    }

    /// The default 1080p baseline and common presets validate.
    #[test]
    fn valid_resolutions_accepted() {
        assert!(validate_resolution(VIDEO_MAX_WIDTH, VIDEO_MAX_HEIGHT).is_ok());
        assert!(validate_resolution(1280, 720).is_ok());
        assert!(validate_resolution(3840, 2160).is_ok()); // 4K is within the edge bound
        assert!(validate_resolution(RESOLUTION_MAX_EDGE, RESOLUTION_MAX_EDGE).is_ok());
    }

    /// Zero, odd, and oversized dimensions are rejected at the boundary.
    #[test]
    fn invalid_resolutions_rejected() {
        assert!(validate_resolution(0, 1080).is_err(), "zero width");
        assert!(validate_resolution(1920, 0).is_err(), "zero height");
        assert!(validate_resolution(1921, 1080).is_err(), "odd width");
        assert!(validate_resolution(1920, 1081).is_err(), "odd height");
        assert!(
            validate_resolution(RESOLUTION_MAX_EDGE + 2, 1080).is_err(),
            "width beyond the edge bound"
        );
        assert!(
            validate_resolution(1920, RESOLUTION_MAX_EDGE + 2).is_err(),
            "height beyond the edge bound"
        );
    }

    /// A fresh failure within budget restarts as attempt 1 with the base backoff.
    #[test]
    fn restart_on_error_within_budget() {
        let d = decide_restart(0, Duration::from_secs(2), EndReason::Error);
        assert_eq!(
            d,
            RestartDecision::Restart {
                attempt: 1,
                delay: BACKOFF_BASE,
            }
        );
        // EOS is treated the same as Error for restart purposes.
        let d = decide_restart(0, Duration::from_secs(2), EndReason::Eos);
        assert!(matches!(d, RestartDecision::Restart { attempt: 1, .. }));
    }

    /// Backoff doubles per attempt and is capped at BACKOFF_CAP.
    #[test]
    fn backoff_doubles_and_caps() {
        let short = Duration::from_secs(1); // below HEALTHY_RUN → counts the budget
        let delay = |used| match decide_restart(used, short, EndReason::Error) {
            RestartDecision::Restart { delay, .. } => delay,
            other => panic!("expected restart, got {other:?}"),
        };
        assert_eq!(delay(0), Duration::from_secs(1)); // attempt 1: 1s
        assert_eq!(delay(1), Duration::from_secs(2)); // attempt 2: 2s
        assert_eq!(delay(2), Duration::from_secs(4)); // attempt 3: 4s
        assert_eq!(delay(3), BACKOFF_CAP); // attempt 4: 8s (= cap)
        assert_eq!(delay(4), BACKOFF_CAP); // attempt 5: would be 16s → capped
    }

    /// Once the attempt budget is exhausted, give up (non-user-quit stop).
    #[test]
    fn give_up_after_max_attempts() {
        let d = decide_restart(MAX_RESTART_ATTEMPTS, Duration::from_secs(1), EndReason::Error);
        assert_eq!(d, RestartDecision::Stop { user_quit: false });
        // Also beyond the max.
        let d = decide_restart(
            MAX_RESTART_ATTEMPTS + 3,
            Duration::from_secs(1),
            EndReason::Eos,
        );
        assert_eq!(d, RestartDecision::Stop { user_quit: false });
    }

    /// A session that ran healthy (≥ HEALTHY_RUN) resets the budget, so even a
    /// previously-exhausted counter restarts again as attempt 1.
    #[test]
    fn counter_resets_after_healthy_run() {
        let healthy = HEALTHY_RUN + Duration::from_secs(5);
        let d = decide_restart(MAX_RESTART_ATTEMPTS, healthy, EndReason::Error);
        assert_eq!(
            d,
            RestartDecision::Restart {
                attempt: 1,
                delay: BACKOFF_BASE,
            },
            "a healthy run should earn a clean restart budget"
        );
        // Exactly at the threshold also counts as healthy.
        let d = decide_restart(MAX_RESTART_ATTEMPTS, HEALTHY_RUN, EndReason::Eos);
        assert!(matches!(d, RestartDecision::Restart { attempt: 1, .. }));
    }

    /// A first-session startup failure stops immediately (non-user-quit) and never
    /// enters the restart/backoff schedule — the caller already has the error.
    #[test]
    fn startup_failure_stops_without_restart() {
        let d = decide_restart(0, Duration::from_millis(5), EndReason::StartupFailed);
        assert_eq!(d, RestartDecision::Stop { user_quit: false });
        // Even a "healthy"-length session that ends as StartupFailed (shouldn't
        // happen, but be defensive) must not restart.
        let d = decide_restart(0, HEALTHY_RUN + Duration::from_secs(1), EndReason::StartupFailed);
        assert_eq!(d, RestartDecision::Stop { user_quit: false });
    }

    /// A user quit always stops cleanly and never restarts — regardless of how
    /// long the session ran or how many attempts remain.
    #[test]
    fn no_restart_on_user_quit() {
        let d = decide_restart(0, Duration::from_secs(1), EndReason::Quit);
        assert_eq!(d, RestartDecision::Stop { user_quit: true });
        // Even with budget fully available and a short run.
        let d = decide_restart(0, Duration::from_millis(10), EndReason::Quit);
        assert_eq!(d, RestartDecision::Stop { user_quit: true });
    }

    /// Drive the same state machine the supervisor loop runs (carry `attempts_used`
    /// forward across failing sessions) and confirm the full trajectory:
    /// 5 restarts with the expected backoffs, then a give-up Stop. This mirrors the
    /// restart→backoff→give-up path without fragile process-level fault injection.
    #[test]
    fn supervisor_trajectory_restart_then_give_up() {
        let short = Duration::from_secs(1); // every session fails fast (unhealthy)
        let mut attempts_used: u32 = 0;
        let mut delays = Vec::new();
        let mut gave_up = false;

        for _ in 0..10 {
            match decide_restart(attempts_used, short, EndReason::Eos) {
                RestartDecision::Restart { attempt, delay } => {
                    attempts_used = attempt; // exactly what the loop does
                    delays.push(delay);
                }
                RestartDecision::Stop { user_quit } => {
                    assert!(!user_quit, "give-up must not be reported as a user quit");
                    gave_up = true;
                    break;
                }
            }
        }

        assert!(gave_up, "supervisor must eventually give up");
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                BACKOFF_CAP, // 8s
                BACKOFF_CAP, // capped
            ],
            "expected {MAX_RESTART_ATTEMPTS} restarts with doubling+capped backoff"
        );
    }

    /// `sleep_interruptible` returns early (false) when quit is already set, and
    /// returns true after a full short sleep when quit stays clear.
    #[test]
    fn sleep_interruptible_honors_quit() {
        let quit = Arc::new(AtomicBool::new(true));
        assert!(
            !sleep_interruptible(Duration::from_secs(5), &quit),
            "should not wait when quit is already set"
        );

        let quit = Arc::new(AtomicBool::new(false));
        let start = Instant::now();
        assert!(sleep_interruptible(Duration::from_millis(150), &quit));
        assert!(
            start.elapsed() >= Duration::from_millis(140),
            "should have waited roughly the full duration"
        );
    }
}
