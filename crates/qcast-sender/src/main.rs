//! Qcast host entry point. By default it shows a small pre-launch GUI (system
//! check + viewer URL/QR); once confirmed, the window CLOSES and Qcast keeps
//! streaming as a background process (no taskbar entry), stoppable with the
//! Ctrl+Alt+Q global hotkey or by killing the process. `--headless` skips the
//! GUI and streams immediately (used for automated tests / running under a
//! service manager).

use anyhow::Result;
use clap::Parser;
use global_hotkey::{
    hotkey::{Code, HotKey, Modifiers},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use gstreamer as gst;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

mod access_code;
mod bundle;
mod capture;
mod gui;
mod host;
mod preflight;
mod turn;

#[derive(Parser, Debug)]
#[command(name = "qcast-sender", about = "Qcast host: serves a desktop stream to browsers")]
struct Args {
    /// Address to bind the web + signalling servers on.
    #[arg(long, default_value = "0.0.0.0")]
    host: String,
    /// Port for the web client (open this in a browser).
    #[arg(long, default_value_t = 8080)]
    web_port: u16,
    /// Port for the WebRTC signalling server.
    #[arg(long, default_value_t = 8443)]
    signalling_port: u16,
    /// Capture source: `auto` (real screen) or `test` (test pattern).
    #[arg(long, default_value = "auto")]
    source: String,
    /// Max captured-frame width (the videoscale cap), e.g. 1920 for 1080p or 1280
    /// for 720p. Values above 1080p may not decode on every browser.
    #[arg(long, default_value_t = host::VIDEO_MAX_WIDTH)]
    max_width: u32,
    /// Max captured-frame height (the videoscale cap), e.g. 1080 for 1080p or 720
    /// for 720p.
    #[arg(long, default_value_t = host::VIDEO_MAX_HEIGHT)]
    max_height: u32,
    /// Video codec preference: `auto` (VP8 preferred), `h264` (H.264 preferred),
    /// `vp8-only`, or `h264-only`.
    #[arg(long, default_value = "auto")]
    codec: String,
    /// Skip the GUI: start streaming immediately and run until Ctrl+C.
    #[arg(long)]
    headless: bool,
}

/// Parse the `--codec` CLI value into a [`host::CodecPref`]. Boundary validation:
/// reject unknown values with a clear message listing the accepted ones.
fn parse_codec_pref(value: &str) -> Result<host::CodecPref> {
    match value {
        "auto" => Ok(host::CodecPref::Auto),
        "h264" => Ok(host::CodecPref::H264Preferred),
        "vp8-only" => Ok(host::CodecPref::Vp8Only),
        "h264-only" => Ok(host::CodecPref::H264Only),
        other => anyhow::bail!(
            "unknown --codec '{other}'; expected one of: auto, h264, vp8-only, h264-only"
        ),
    }
}

fn main() -> Result<()> {
    // App-focused logging; silence the chatty GPU/winit/signalling layers unless
    // the user opts in via RUST_LOG, so a first run isn't full of scary warnings.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,wgpu_hal=off,wgpu_core=off,naga=off,egui_wgpu=warn,egui_winit=warn,\
             gst_plugin_webrtc_signalling=warn",
        )
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
    // Relocatable bundles: point GStreamer at plugins shipped next to the exe
    // BEFORE init (GStreamer reads these env vars only at init time). No-op for a
    // normal dev build, so `cargo run` keeps using the system/user plugin path.
    bundle::configure_plugin_path();
    gst::init().expect("failed to initialize GStreamer");
    let args = Args::parse();

    // Validate operator-supplied options at the boundary, before they reach the
    // pipeline. Fail fast with a clear message.
    host::validate_resolution(args.max_width, args.max_height)?;
    let codec_pref = parse_codec_pref(&args.codec)?;

    // Generate the viewer access code ONCE here: this is the single source of
    // truth shared by the GUI display, the headless log line, and the served
    // session.json the browser reads.
    let access_code = access_code::generate();

    let cfg = host::HostConfig {
        host: args.host.clone(),
        web_port: args.web_port,
        signalling_port: args.signalling_port,
        test_pattern: args.source == "test",
        max_width: args.max_width,
        max_height: args.max_height,
        codec_pref,
        access_code,
    };

    // One quit signal for the whole process: Ctrl+C / SIGTERM flips it (and wakes
    // the GUI event loop if it's up). The universal "kill it" path the operator
    // can always rely on, alongside the global hotkey.
    let quit = Arc::new(AtomicBool::new(false));
    {
        let q = quit.clone();
        if let Err(e) = ctrlc::set_handler(move || {
            q.store(true, Ordering::SeqCst);
            gui::wake();
        }) {
            // Non-fatal: the global hotkey and a hard kill still stop the process,
            // but Ctrl+C / SIGTERM won't shut down cleanly without this handler.
            tracing::warn!(error = %e, "could not install Ctrl+C/SIGTERM handler; use Ctrl+Alt+Q or kill to stop");
        }
    }

    if args.headless {
        run_headless(cfg, quit)
    } else {
        match gui::run(cfg, quit.clone())? {
            gui::Outcome::Quit => Ok(()),
            gui::Outcome::Background(host) => run_background(host, quit),
        }
    }
}

/// Headless path: fail fast on a missing streaming core, start, then block until
/// interrupted (Ctrl+C / SIGTERM).
fn run_headless(cfg: host::HostConfig, quit: Arc<AtomicBool>) -> Result<()> {
    if gst::ElementFactory::find("webrtcsink").is_none() {
        anyhow::bail!(
            "webrtcsink not found — build & install the gst-plugins-rs webrtc plugin \
             (see deploy/setup-linux.sh)"
        );
    }
    let code = cfg.access_code.clone();
    let mut running = host::start(cfg)?;
    tracing::info!("qcast host serving — open  {}  on any device", running.url);
    tracing::info!("viewer password: {}", code);
    while !quit.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    tracing::info!("shutting down");
    running.stop();
    Ok(())
}

/// After the GUI window closes, keep the process alive headless (no window, no
/// taskbar) until the global hotkey or a kill/Ctrl+C stops it.
fn run_background(mut host: host::RunningHost, quit: Arc<AtomicBool>) -> Result<()> {
    tracing::info!("Qcast is now running in the background — open  {}  on any device", host.url);
    let (_mgr, hotkey_id) = register_quit_hotkey();
    tracing::info!(
        "stop with Ctrl+Alt+Q, or kill this process (pid {})",
        std::process::id()
    );

    loop {
        if quit.load(Ordering::SeqCst) {
            break;
        }
        if let Some(id) = hotkey_id {
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    quit.store(true, Ordering::SeqCst);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    tracing::info!("stopping");
    host.stop();
    Ok(())
}

/// Register Ctrl+Alt+Q as the global quit hotkey. Returns `(manager, id)`; the
/// manager must be kept alive. Both are `None`/`None` where the platform can't
/// grab a global hotkey (e.g. Wayland) — killing the process is the fallback.
fn register_quit_hotkey() -> (Option<GlobalHotKeyManager>, Option<u32>) {
    let mgr = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "global hotkey unavailable; use kill / Ctrl+C to stop");
            return (None, None);
        }
    };
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyQ);
    let id = hotkey.id();
    match mgr.register(hotkey) {
        Ok(()) => (Some(mgr), Some(id)),
        Err(e) => {
            tracing::warn!(error = %e, "could not register Ctrl+Alt+Q; use kill / Ctrl+C to stop");
            (Some(mgr), None)
        }
    }
}
