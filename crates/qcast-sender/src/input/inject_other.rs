//! Non-Windows injector: log the decoded input instead of replaying it.
//!
//! Windows is the real injection target (`SendInput`). On the Linux dev box we only
//! want to exercise the decode + transport path without taking over the developer's
//! own desktop, so this backend just logs each decoded event. A real Linux backend
//! (XTEST / `uinput` / the Wayland `RemoteDesktop` portal) is deferred with Linux
//! support (see the remote-support pivot notes).

use super::{InputEvent, InputInjector};

#[derive(Default)]
pub struct LoggingInjector;

impl InputInjector for LoggingInjector {
    fn inject(&mut self, event: &InputEvent) {
        tracing::info!(?event, "input: decoded remote event (no-op injector on this platform)");
    }
}
