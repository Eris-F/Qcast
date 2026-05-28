//! Receiver→sender remote-control input: decode `GstNavigation` events that arrive
//! over webrtcsink's navigation data channel and replay them on this machine.
//!
//! Wiring: [`attach_navigation_probes`] puts an upstream-event probe on each
//! webrtcsink sink pad; each navigation event is decoded by
//! [`InputEvent::from_navigation`] and handed to an [`InputInjector`]. The real OS
//! injection (Windows `SendInput`) lives behind `#[cfg(windows)]`; other targets get
//! a logging no-op so the decode + dispatch path runs — and is tested — on the Linux
//! dev box without taking over the developer's own session.

mod event;
mod inject_file;
pub mod key;
#[cfg(not(windows))]
mod inject_other;
#[cfg(windows)]
mod inject_windows;

pub use event::InputEvent;
// Part of the public input API (named by `InputEvent::MouseButton`), but only
// referenced by the cfg(windows) SendInput backend + tests — hence the allow on
// non-Windows builds.
#[allow(unused_imports)]
pub use event::MouseButton;

use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::{Arc, Mutex};

/// Replays a decoded [`InputEvent`] on this machine.
pub trait InputInjector: Send {
    fn inject(&mut self, event: &InputEvent);
}

/// Shared, thread-safe injector — probes fire on GStreamer streaming threads.
pub type SharedInjector = Arc<Mutex<Box<dyn InputInjector>>>;

/// Build the input injector. If `QCAST_INPUT_LOG=<path>` is set, decoded events are
/// appended to that file (for the automatable browser→sender E2E — see
/// `deploy/TEST_PLAN.md`) instead of being replayed. Otherwise: Windows `SendInput`,
/// else a logging no-op.
pub fn platform_injector() -> Box<dyn InputInjector> {
    if let Some(path) = std::env::var_os("QCAST_INPUT_LOG") {
        match inject_file::FileLoggingInjector::new(&path) {
            Ok(inj) => {
                tracing::info!(path = ?path, "input: logging decoded events to QCAST_INPUT_LOG file");
                return Box::new(inj);
            }
            Err(e) => tracing::warn!(
                error = %e,
                "input: QCAST_INPUT_LOG set but could not open the file; using the platform injector"
            ),
        }
    }
    #[cfg(windows)]
    {
        Box::new(inject_windows::SendInputInjector::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(inject_other::LoggingInjector::default())
    }
}

/// A ready-to-share platform injector.
pub fn shared_injector() -> SharedInjector {
    Arc::new(Mutex::new(platform_injector()))
}

/// Decode one (upstream) event and, if it is an actionable input event, inject it;
/// returns whether it was. Split out from the probe closure so it is unit-testable
/// without a live data channel. `frame` is the negotiated frame size `(w, h)` used
/// to normalize pointer coordinates.
pub fn dispatch_event(event: &gst::EventRef, frame: (f64, f64), injector: &SharedInjector) -> bool {
    match InputEvent::from_navigation(event, frame) {
        Some(input) => {
            if let Ok(mut inj) = injector.lock() {
                inj.inject(&input);
            }
            true
        }
        None => false,
    }
}

/// Attach an upstream-event probe to every sink pad of `webrtcsink`, so navigation
/// events the receiver sends over the data channel are decoded + injected. Returns
/// the number of pads probed; **0 means no sink pad was present yet**, a wiring
/// problem worth logging.
pub fn attach_navigation_probes(
    webrtcsink: &gst::Element,
    frame: (f64, f64),
    injector: SharedInjector,
) -> usize {
    let pads = webrtcsink.sink_pads();
    for pad in &pads {
        let injector = injector.clone();
        pad.add_probe(gst::PadProbeType::EVENT_UPSTREAM, move |_pad, info| {
            if let Some(gst::PadProbeData::Event(ref event)) = info.data {
                dispatch_event(event, frame, &injector);
            }
            gst::PadProbeReturn::Ok
        });
    }
    pads.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test injector that records what it was asked to inject.
    struct CaptureInjector(Arc<Mutex<Vec<InputEvent>>>);
    impl InputInjector for CaptureInjector {
        fn inject(&mut self, event: &InputEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    fn capture() -> (SharedInjector, Arc<Mutex<Vec<InputEvent>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let injector: SharedInjector = Arc::new(Mutex::new(Box::new(CaptureInjector(log.clone()))));
        (injector, log)
    }

    #[test]
    fn dispatch_decodes_and_injects_navigation() {
        gst::init().unwrap();
        let (injector, log) = capture();

        let mv = gstreamer_video::NavigationEvent::new_mouse_move(960.0, 540.0).build();
        assert!(dispatch_event(&mv, (1920.0, 1080.0), &injector));
        let key = gstreamer_video::NavigationEvent::new_key_press("a").build();
        assert!(dispatch_event(&key, (1920.0, 1080.0), &injector));

        assert_eq!(
            log.lock().unwrap().as_slice(),
            &[
                InputEvent::MouseMove { x: 0.5, y: 0.5 },
                InputEvent::Key { key: "a".into(), pressed: true },
            ]
        );
    }

    #[test]
    fn dispatch_ignores_non_navigation() {
        gst::init().unwrap();
        let (injector, log) = capture();
        assert!(!dispatch_event(&gst::event::Eos::new(), (1920.0, 1080.0), &injector));
        assert!(log.lock().unwrap().is_empty());
    }

    /// `attach_navigation_probes` finds the element's sink pad(s). Guards the
    /// "webrtcsink had no sink pad" regression (which would silently disable
    /// remote control) using a plain `fakesink` — no `webrtcsink` plugin needed.
    #[test]
    fn attach_probes_reports_the_sink_pad_count() {
        gst::init().unwrap();
        let sink = gst::ElementFactory::make("fakesink").build().unwrap();
        let (injector, _log) = capture();
        assert_eq!(attach_navigation_probes(&sink, (1920.0, 1080.0), injector), 1);
    }

    /// End-to-end wiring: an upstream `GstNavigation` event sent into a live pad
    /// traverses the `EVENT_UPSTREAM` probe, is decoded, and is injected. This is
    /// the regression net for the probe ↔ dispatch ↔ injector path (the part the
    /// pure decode unit tests can't reach). Uses `videotestsrc ! fakesink`, so it
    /// runs anywhere GStreamer core elements exist (no `webrtcsink` required).
    #[test]
    fn upstream_navigation_event_reaches_the_injector() {
        gst::init().unwrap();
        let pipeline = gst::parse::launch("videotestsrc ! fakesink name=fs")
            .unwrap()
            .downcast::<gst::Pipeline>()
            .unwrap();
        let fs = pipeline.by_name("fs").unwrap();

        let (injector, log) = capture();
        assert_eq!(attach_navigation_probes(&fs, (1920.0, 1080.0), injector), 1);

        // Activate the pad so an upstream event can traverse it, then send one.
        pipeline.set_state(gst::State::Paused).unwrap();
        let _ = pipeline.state(Some(gst::ClockTime::from_seconds(5)));
        let nav = gstreamer_video::NavigationEvent::new_mouse_button_press(1, 1920.0, 540.0).build();
        fs.send_event(nav);
        pipeline.set_state(gst::State::Null).unwrap();

        assert_eq!(
            log.lock().unwrap().as_slice(),
            &[InputEvent::MouseButton {
                button: MouseButton::Left,
                x: 1.0,
                y: 0.5,
                pressed: true,
            }]
        );
    }

    /// The QCAST_INPUT_LOG file injector appends decoded events — the sink the
    /// future browser→sender E2E asserts against (deploy/TEST_PLAN.md, Layer 4).
    #[test]
    fn file_logging_injector_appends_events() {
        use std::io::Read;
        let path = std::env::temp_dir().join(format!("qcast-input-log-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut inj = inject_file::FileLoggingInjector::new(&path).unwrap();
            inj.inject(&InputEvent::MouseMove { x: 0.5, y: 0.25 });
            inj.inject(&InputEvent::Key { key: "a".into(), pressed: true });
        }
        let mut s = String::new();
        std::fs::File::open(&path).unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("MouseMove { x: 0.5, y: 0.25 }"), "log was: {s:?}");
        assert!(s.contains(r#"Key { key: "a", pressed: true }"#), "log was: {s:?}");
        let _ = std::fs::remove_file(&path);
    }
}
