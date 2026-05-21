//! Wayland desktop capture via the xdg-desktop-portal ScreenCast portal
//! (`ashpd`). Returns a live PipeWire remote fd + node id that `pipewiresrc`
//! consumes. The portal shows a picker the host operator approves once.
//!
//! The portal proxy, session, and fd are intentionally leaked: the host needs
//! the capture for its entire lifetime, and dropping any of them tears the
//! stream down. One bounded leak per host process is acceptable.

use anyhow::{Context, Result};
use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use std::os::fd::IntoRawFd;

/// Acquire a screen-capture stream. Returns `(pipewire_fd, node_id)` for
/// `pipewiresrc`. Triggers the portal's screen-picker dialog.
pub async fn acquire() -> Result<(i32, u32)> {
    let proxy: &'static Screencast = Box::leak(Box::new(
        Screencast::new().await.context("connect to ScreenCast portal")?,
    ));

    let session = proxy
        .create_session(Default::default())
        .await
        .context("create portal session")?;

    proxy
        .select_sources(
            &session,
            SelectSourcesOptions::default()
                .set_cursor_mode(CursorMode::Embedded)
                .set_sources(SourceType::Monitor | SourceType::Window)
                .set_multiple(false)
                .set_persist_mode(PersistMode::DoNot),
        )
        .await
        .context("select sources")?;

    let streams = proxy
        .start(&session, None, Default::default())
        .await
        .context("start screencast (approve the screen-share dialog)")?
        .response()
        .context("screencast portal response")?;

    let node_id = streams
        .streams()
        .first()
        .context("portal returned no streams")?
        .pipe_wire_node_id();

    let fd = proxy
        .open_pipe_wire_remote(&session, Default::default())
        .await
        .context("open pipewire remote")?;

    // Keep the session alive for the whole process (see module docs).
    let _ = Box::leak(Box::new(session));
    Ok((fd.into_raw_fd(), node_id))
}
