//! Platform desktop capture. Produces the GStreamer source sub-pipeline string
//! that feeds the rest of the host pipeline (`… ! videoconvert ! videoscale …`).
//!
//! - **Linux/Wayland (+ modern X11):** the xdg-desktop-portal ScreenCast portal
//!   (`ashpd`) hands us a live PipeWire fd + node id that `pipewiresrc` consumes.
//!   The portal shows a one-time picker the host operator approves.
//! - **Linux/X11 fallback:** `ximagesrc` if the portal is unavailable.
//! - **Windows:** `d3d11screencapturesrc` (Desktop Duplication), downloaded to
//!   system memory for the software/VAAPI/QSV encoder webrtcsink picks.
//!
//! On Linux the portal proxy, session, and fd are intentionally leaked: the host
//! needs the capture for its entire lifetime, and dropping any of them tears the
//! stream down. One bounded leak per host process is acceptable.

use anyhow::Result;

/// Build the capture source sub-pipeline for this platform. `rt` is the tokio
/// runtime kept alive by the caller (needed for the async portal handshake).
#[cfg(target_os = "linux")]
pub fn source_description(rt: &tokio::runtime::Runtime) -> Result<String> {
    use gstreamer as gst;
    match rt.block_on(acquire()) {
        Ok((fd, node_id)) => {
            tracing::info!(fd, node_id, "capturing desktop via xdg-desktop-portal");
            Ok(format!("pipewiresrc fd={fd} path={node_id}"))
        }
        Err(e) => {
            // No portal (e.g. headless X11 / no desktop-portal service): fall back
            // to ximagesrc if the X11 capture element is present.
            if gst::ElementFactory::find("ximagesrc").is_some() {
                tracing::warn!(error = ?e, "portal capture unavailable; falling back to ximagesrc");
                Ok("ximagesrc use-damage=false ! videoconvert".to_string())
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub fn source_description(_rt: &tokio::runtime::Runtime) -> Result<String> {
    // Desktop Duplication via Direct3D 11, then download GPU frames to system
    // memory so the negotiated encoder (software VP8, QSV/MF H.264, …) can read
    // them. Windows capture path is not yet validated at runtime.
    tracing::info!("capturing desktop via d3d11screencapturesrc");
    Ok("d3d11screencapturesrc show-cursor=true ! d3d11download ! videoconvert".to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn source_description(_rt: &tokio::runtime::Runtime) -> Result<String> {
    anyhow::bail!("desktop capture is not implemented for this platform yet")
}

/// Acquire a screen-capture stream via xdg-desktop-portal. Returns
/// `(pipewire_fd, node_id)` for `pipewiresrc`. Triggers the portal picker dialog.
#[cfg(target_os = "linux")]
async fn acquire() -> Result<(i32, u32)> {
    use anyhow::Context;
    use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
    use ashpd::desktop::PersistMode;
    use std::os::fd::IntoRawFd;

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
