//! The decoded remote-control input model.
//!
//! The receiver (in its WebView2) sends mouse/keyboard as `GstNavigation` events
//! over webrtcsink's navigation data channel. On the sender they surface as
//! upstream `GstNavigation` events; we decode them into [`InputEvent`] — our own
//! representation, intentionally decoupled from `gstreamer_video`'s versioned
//! `NavigationEvent` enum so the injection backends and any future cross-machine
//! protocol stay stable.

use gstreamer as gst;
use gstreamer_video::NavigationEvent;

/// A pointer button. `GstNavigation` numbers buttons 1=left, 2=middle, 3=right
/// (the X11 / web `MouseEvent.button+1` convention).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Other(i32),
}

impl MouseButton {
    fn from_nav(button: i32) -> Self {
        match button {
            1 => Self::Left,
            2 => Self::Middle,
            3 => Self::Right,
            other => Self::Other(other),
        }
    }
}

/// One decoded input action to replay on this (the controlled/sender) machine.
///
/// Pointer coordinates are **normalized to `0.0..=1.0`** of the streamed frame, so a
/// backend can map them onto the local screen without knowing the capture
/// resolution. (Letterbox from `add-borders` is not yet compensated — see the
/// Windows backend TODO.)
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Move the pointer to a normalized position.
    MouseMove { x: f64, y: f64 },
    /// Press or release a pointer button at a normalized position.
    MouseButton {
        button: MouseButton,
        x: f64,
        y: f64,
        pressed: bool,
    },
    /// Scroll the wheel at a normalized pointer position. `dx`/`dy` are the raw
    /// `GstNavigation` scroll deltas (`dy > 0` conventionally scrolls down/toward
    /// the user).
    MouseScroll { x: f64, y: f64, dx: f64, dy: f64 },
    /// Press or release a key. `key` is the `GstNavigation` key string: a single
    /// character for printable keys (`"a"`, `"@"`) or an X11 keysym name for
    /// control keys (`"Return"`, `"BackSpace"`, `"Left"`).
    Key { key: String, pressed: bool },
}

impl InputEvent {
    /// Decode a `GstNavigation` event into our model, normalizing pointer
    /// coordinates against the negotiated `frame` size `(width, height)` in pixels.
    /// Returns `None` for non-navigation events and for navigation kinds we don't
    /// act on yet (scroll, touch, commands, double-click).
    pub fn from_navigation(event: &gst::EventRef, frame: (f64, f64)) -> Option<Self> {
        // `parse` self-checks the event type and errors on non-navigation events.
        let nav = NavigationEvent::parse(event).ok()?;
        let (fw, fh) = frame;
        let nx = |x: f64| if fw > 0.0 { (x / fw).clamp(0.0, 1.0) } else { 0.0 };
        let ny = |y: f64| if fh > 0.0 { (y / fh).clamp(0.0, 1.0) } else { 0.0 };

        // `{ .. }` patterns stay valid regardless of which gstreamer-video version
        // features add fields (e.g. `modifier_state` in v1_22), so we don't pin one.
        Some(match nav {
            NavigationEvent::MouseMove { x, y, .. } => Self::MouseMove { x: nx(x), y: ny(y) },
            NavigationEvent::MouseButtonPress { button, x, y, .. } => Self::MouseButton {
                button: MouseButton::from_nav(button),
                x: nx(x),
                y: ny(y),
                pressed: true,
            },
            NavigationEvent::MouseButtonRelease { button, x, y, .. } => Self::MouseButton {
                button: MouseButton::from_nav(button),
                x: nx(x),
                y: ny(y),
                pressed: false,
            },
            NavigationEvent::MouseScroll { x, y, delta_x, delta_y, .. } => Self::MouseScroll {
                x: nx(x),
                y: ny(y),
                dx: delta_x,
                dy: delta_y,
            },
            NavigationEvent::KeyPress { key, .. } => Self::Key { key, pressed: true },
            NavigationEvent::KeyRelease { key, .. } => Self::Key { key, pressed: false },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: (f64, f64) = (1920.0, 1080.0);

    fn init() {
        gst::init().unwrap();
    }

    #[test]
    fn normalizes_mouse_move_to_unit_square() {
        init();
        let ev = NavigationEvent::new_mouse_move(960.0, 540.0).build();
        assert_eq!(
            InputEvent::from_navigation(&ev, FRAME),
            Some(InputEvent::MouseMove { x: 0.5, y: 0.5 })
        );
    }

    #[test]
    fn decodes_button_press_and_release() {
        init();
        let press = NavigationEvent::new_mouse_button_press(1, 0.0, 0.0).build();
        assert_eq!(
            InputEvent::from_navigation(&press, FRAME),
            Some(InputEvent::MouseButton {
                button: MouseButton::Left,
                x: 0.0,
                y: 0.0,
                pressed: true,
            })
        );
        let release = NavigationEvent::new_mouse_button_release(3, 1920.0, 1080.0).build();
        assert_eq!(
            InputEvent::from_navigation(&release, FRAME),
            Some(InputEvent::MouseButton {
                button: MouseButton::Right,
                x: 1.0,
                y: 1.0,
                pressed: false,
            })
        );
    }

    #[test]
    fn decodes_key_press_and_release() {
        init();
        let press = NavigationEvent::new_key_press("a").build();
        assert_eq!(
            InputEvent::from_navigation(&press, FRAME),
            Some(InputEvent::Key { key: "a".into(), pressed: true })
        );
        let release = NavigationEvent::new_key_release("Return").build();
        assert_eq!(
            InputEvent::from_navigation(&release, FRAME),
            Some(InputEvent::Key { key: "Return".into(), pressed: false })
        );
    }

    #[test]
    fn decodes_mouse_scroll() {
        init();
        let ev = NavigationEvent::new_mouse_scroll(960.0, 540.0, 0.0, -3.0).build();
        assert_eq!(
            InputEvent::from_navigation(&ev, FRAME),
            Some(InputEvent::MouseScroll { x: 0.5, y: 0.5, dx: 0.0, dy: -3.0 })
        );
    }

    #[test]
    fn ignores_non_navigation_events() {
        init();
        let eos = gst::event::Eos::new();
        assert_eq!(InputEvent::from_navigation(&eos, FRAME), None);
    }

    #[test]
    fn clamps_out_of_range_coordinates() {
        init();
        let ev = NavigationEvent::new_mouse_move(3000.0, -50.0).build();
        assert_eq!(
            InputEvent::from_navigation(&ev, FRAME),
            Some(InputEvent::MouseMove { x: 1.0, y: 0.0 })
        );
    }
}
