//! Windows remote-control injection via `SendInput`.
//!
//! ⚠️ **Authored on the Linux dev box; only compiles under `#[cfg(windows)]` and has
//! NOT been built or run on Windows yet.** Validate on the Windows boot — see the
//! test plan in `deploy/TEST_PLAN.md` and the checklist in `deploy/WINDOWS_INSTALLER.md`.
//!
//! Design (matches the remote-support pivot):
//! - **Keyboard is layout-proof.** Printable characters are injected as Unicode via
//!   `KEYEVENTF_UNICODE` (so the receiver's "produced character" lands regardless of
//!   the sender's keyboard layout); control/navigation keys map to a small set of
//!   virtual-key codes with proper press/release semantics.
//! - **Mouse is absolute.** Normalized `0.0..=1.0` coordinates map to the primary
//!   monitor via `MOUSEEVENTF_ABSOLUTE` (0..65535). Multi-monitor (`VIRTUALDESK`)
//!   and `add-borders` letterbox compensation are TODOs to settle during validation.

use super::{InputEvent, InputInjector, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};

/// `SendInput`-based injector. Stateless — every event is a fresh `SendInput` call.
pub struct SendInputInjector;

impl SendInputInjector {
    pub fn new() -> Self {
        Self
    }
}

impl InputInjector for SendInputInjector {
    fn inject(&mut self, event: &InputEvent) {
        match event {
            InputEvent::MouseMove { x, y } => send_mouse(*x, *y, MOUSEEVENTF_MOVE),
            InputEvent::MouseButton { button, x, y, pressed } => {
                send_mouse(*x, *y, MOUSEEVENTF_MOVE | button_flag(*button, *pressed));
            }
            InputEvent::Key { key, pressed } => send_key(key, *pressed),
        }
    }
}

/// Map a normalized fraction `0.0..=1.0` to the `SendInput` absolute range `0..65535`.
fn to_absolute(fraction: f64) -> i32 {
    (fraction.clamp(0.0, 1.0) * 65535.0).round() as i32
}

fn send_mouse(x: f64, y: f64, flags: MOUSE_EVENT_FLAGS) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: to_absolute(x),
                dy: to_absolute(y),
                mouseData: 0,
                dwFlags: flags | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}

/// Down/up flag for a pointer button. `Other(_)` buttons are not injected (no flag).
fn button_flag(button: MouseButton, pressed: bool) -> MOUSE_EVENT_FLAGS {
    match (button, pressed) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Other(_), _) => MOUSE_EVENT_FLAGS(0),
    }
}

/// Map a `GstNavigation` key string (X11 keysym name) to a Windows virtual key, for
/// the control/navigation keys that can't be expressed as a typed Unicode character.
fn named_key(key: &str) -> Option<VIRTUAL_KEY> {
    Some(match key {
        "Return" | "KP_Enter" => VK_RETURN,
        "BackSpace" => VK_BACK,
        "Tab" => VK_TAB,
        "Escape" => VK_ESCAPE,
        "space" => VK_SPACE,
        "Delete" => VK_DELETE,
        "Left" => VK_LEFT,
        "Right" => VK_RIGHT,
        "Up" => VK_UP,
        "Down" => VK_DOWN,
        "Home" => VK_HOME,
        "End" => VK_END,
        "Page_Up" => VK_PRIOR,
        "Page_Down" => VK_NEXT,
        _ => return None,
    })
}

fn send_key(key: &str, pressed: bool) {
    // Control/navigation keys: inject the virtual key with real press/release.
    if let Some(vk) = named_key(key) {
        send_vk(vk, pressed);
        return;
    }
    // Printable text: type each UTF-16 unit as Unicode on the *press* only (Unicode
    // injection is "produce this character"; a paired release would double it).
    if pressed {
        for unit in key.encode_utf16() {
            send_unicode_unit(unit, true);
            send_unicode_unit(unit, false);
        }
    }
}

fn send_vk(vk: VIRTUAL_KEY, pressed: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if pressed { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}

fn send_unicode_unit(unit: u16, down: bool) {
    let flags = if down { KEYEVENTF_UNICODE } else { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
}
