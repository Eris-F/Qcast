//! Runtime, platform- and component-agnostic selection of GStreamer elements,
//! always with a software fallback so Qcast runs on any machine GStreamer
//! supports. Hardware encoders/decoders are used when the host's drivers expose
//! them; otherwise we fall back to the bundled/installed software codec.
//!
//! [`gstreamer::init`] must be called before any of these — factory lookup
//! needs the plugin registry.

use gstreamer::ElementFactory;

/// Returns the factory name of the first element in `candidates` present in the
/// local GStreamer registry, or `None` if none are available.
fn first_available(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|name| ElementFactory::find(name).is_some())
        .map(|name| (*name).to_string())
}

/// Best available H.264 encoder: hardware first, software fallback last.
pub fn pick_h264_encoder() -> Option<String> {
    first_available(&[
        "nvh264enc",   // NVIDIA NVENC
        "vah264lpenc", // Intel VAAPI low-power (preferred on Intel iGPU)
        "vah264enc",   // VAAPI
        "qsvh264enc",  // Intel QuickSync (Windows)
        "mfh264enc",   // Windows Media Foundation
        "vtenc_h264",  // macOS VideoToolbox
        "openh264enc", // software (bundled/installed fallback)
        "x264enc",     // software (universal)
    ])
}

/// Best available H.264 decoder: hardware first, software fallback last.
pub fn pick_h264_decoder() -> Option<String> {
    first_available(&[
        "nvh264dec",
        "vah264dec",
        "d3d11h264dec",
        "vtdec",
        "openh264dec",
        "avdec_h264",
    ])
}

/// Best available screen-capture source for the current platform.
pub fn pick_screen_source() -> Option<String> {
    first_available(&[
        "pipewiresrc",           // Wayland (via xdg-desktop-portal)
        "d3d11screencapturesrc", // Windows
        "wgcsrc",                // Windows (Graphics Capture)
        "avfvideosrc",           // macOS
        "ximagesrc",             // X11
    ])
}

/// A snapshot of what was selected on this machine, for logging at startup.
#[derive(Debug, Clone)]
pub struct Selection {
    pub source: Option<String>,
    pub encoder: Option<String>,
    pub decoder: Option<String>,
}

/// Probe the registry for the elements Qcast needs on this machine.
pub fn probe() -> Selection {
    Selection {
        source: pick_screen_source(),
        encoder: pick_h264_encoder(),
        decoder: pick_h264_decoder(),
    }
}

/// Elements `webrtcbin` needs at runtime to set up an encrypted media transport
/// (ICE via `nice`, DTLS, SRTP, RTP management). Returns the names of any that
/// are missing on this machine, so the host can fail fast with a clear message.
pub fn missing_webrtc_support() -> Vec<&'static str> {
    ["webrtcbin", "nicesink", "dtlsenc", "srtpenc", "rtpbin"]
        .into_iter()
        .filter(|name| ElementFactory::find(name).is_none())
        .collect()
}
