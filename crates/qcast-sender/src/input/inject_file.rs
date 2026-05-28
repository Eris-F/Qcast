//! A cross-platform injector that appends each decoded [`InputEvent`] (Debug-
//! formatted, one per line) to a file instead of replaying it. Selected at startup
//! by setting `QCAST_INPUT_LOG=<path>`.
//!
//! Purpose: make the browser→sender navigation path **automatable end-to-end** — a
//! Playwright test drives the receiver UI, then asserts this file's contents —
//! without real desktop side effects. See `deploy/TEST_PLAN.md` (Layer 4).

use super::{InputEvent, InputInjector};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub struct FileLoggingInjector {
    file: File,
}

impl FileLoggingInjector {
    pub fn new(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file })
    }
}

impl InputInjector for FileLoggingInjector {
    fn inject(&mut self, event: &InputEvent) {
        // Best-effort test sink: a write error just drops the line.
        let _ = writeln!(self.file, "{event:?}");
        let _ = self.file.flush();
    }
}
