//! Qcast host entry point. By default it shows a small pre-launch GUI (system
//! check + viewer URL/QR) and, once confirmed, hides itself and streams in the
//! background. `--headless` skips the GUI and streams immediately (used for
//! automated tests and for running under a service manager).

use anyhow::Result;
use clap::Parser;
use gstreamer as gst;

mod capture;
mod gui;
mod host;
mod preflight;

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
    /// Skip the GUI: start streaming immediately and run until Ctrl+C.
    #[arg(long)]
    headless: bool,
}

fn main() -> Result<()> {
    // Default to a clean, app-focused log. The GPU/winit stack (wgpu/naga) is
    // chatty about probing ICDs it then discards — silence it unless the user
    // opts in via RUST_LOG, so a first run isn't full of scary-looking warnings.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(
            "info,wgpu_hal=off,wgpu_core=off,naga=off,egui_wgpu=warn,egui_winit=warn,\
             gst_plugin_webrtc_signalling=warn",
        )
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
    gst::init().expect("failed to initialize GStreamer");
    let args = Args::parse();

    let cfg = host::HostConfig {
        host: args.host.clone(),
        web_port: args.web_port,
        signalling_port: args.signalling_port,
        test_pattern: args.source == "test",
    };

    if args.headless {
        run_headless(cfg)
    } else {
        gui::run(cfg)
    }
}

/// Headless path: fail fast on a missing streaming core, start, then block until
/// the operator interrupts (Ctrl+C / SIGTERM).
fn run_headless(cfg: host::HostConfig) -> Result<()> {
    if gst::ElementFactory::find("webrtcsink").is_none() {
        anyhow::bail!(
            "webrtcsink not found — build & install the gst-plugins-rs webrtc plugin \
             (see deploy/setup-linux.sh)"
        );
    }

    let mut running = host::start(cfg)?;
    tracing::info!("qcast host serving — open  {}  on any device", running.url);

    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .ok();
    let _ = rx.recv();

    tracing::info!("shutting down");
    running.stop();
    Ok(())
}
