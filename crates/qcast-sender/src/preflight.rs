//! Startup self-check: does this machine have everything Qcast needs to stream?
//! Rendered as a checklist in the GUI so the operator sees a clear green/red
//! picture (and a pointer to the setup script) before committing to launch.

use gstreamer as gst;
use qcast_core::elements;

/// One line item in the preflight checklist.
#[derive(Clone)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    /// Short human detail: what was found, or how to fix it.
    pub detail: String,
    /// Critical checks must pass before streaming can start; non-critical are
    /// informational (e.g. which hardware encoder was found).
    pub critical: bool,
}

/// The full preflight result plus the viewer URL to display.
pub struct Report {
    pub checks: Vec<Check>,
    pub url: String,
}

impl Report {
    /// True when every *critical* check passed — i.e. streaming can start.
    pub fn ready(&self) -> bool {
        self.checks.iter().all(|c| !c.critical || c.ok)
    }
}

fn have(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Run all checks. `gst::init()` must already have been called.
pub fn run(host: &str, web_port: u16) -> Report {
    let (lan_ip, url) = crate::host::lan_url(host, web_port);
    let mut checks = Vec::new();

    // --- critical: the streaming core ---
    checks.push(Check {
        name: "Streaming plugin (webrtcsink)".into(),
        ok: have("webrtcsink"),
        detail: if have("webrtcsink") {
            "found".into()
        } else {
            "missing — run the Qcast setup script to build & install it".into()
        },
        critical: true,
    });

    let webrtc_missing = elements::missing_webrtc_support();
    checks.push(Check {
        name: "WebRTC transport (ICE/DTLS/SRTP)".into(),
        ok: webrtc_missing.is_empty(),
        detail: if webrtc_missing.is_empty() {
            "found".into()
        } else {
            format!("missing: {}", webrtc_missing.join(", "))
        },
        critical: true,
    });

    let have_vp8 = have("vp8enc");
    checks.push(Check {
        name: "Video encoder (VP8)".into(),
        ok: have_vp8,
        detail: if have_vp8 {
            "vp8enc found".into()
        } else {
            "missing — install gstreamer plugins-good".into()
        },
        critical: true,
    });

    let source = elements::pick_screen_source();
    checks.push(Check {
        name: "Screen capture source".into(),
        ok: source.is_some(),
        detail: source
            .clone()
            .unwrap_or_else(|| "none available for this platform".into()),
        critical: true,
    });

    checks.push(Check {
        name: "Network address".into(),
        ok: lan_ip != "0.0.0.0",
        detail: if lan_ip != "0.0.0.0" {
            url.clone()
        } else {
            "could not determine a LAN IP".into()
        },
        critical: true,
    });

    // --- informational: hardware acceleration ---
    let hw_enc = elements::pick_h264_encoder();
    checks.push(Check {
        name: "Hardware H.264 encoder".into(),
        ok: hw_enc
            .as_deref()
            .map(|e| e != "openh264enc" && e != "x264enc")
            .unwrap_or(false),
        detail: hw_enc.unwrap_or_else(|| "none (software encode only)".into()),
        critical: false,
    });

    Report { checks, url }
}
