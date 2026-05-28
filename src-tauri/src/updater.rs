//! Phase 5 — real implementations behind the `check_for_updates` /
//! `apply_update` Tauri commands.
//!
//! Phase 4 (the IPC layer) ships placeholder `Ok(None)` / `Ok(())` stubs in
//! `src-tauri/src/commands.rs`. The merge step replaces those stub bodies with
//! calls into this module, e.g.
//!
//! ```ignore
//! #[tauri::command]
//! async fn check_for_updates(app: tauri::AppHandle) -> Result<Option<crate::updater::UpdateInfo>, String> {
//!     crate::updater::check(app).await
//! }
//!
//! #[tauri::command]
//! async fn apply_update(app: tauri::AppHandle) -> Result<(), String> {
//!     crate::updater::apply(app).await
//! }
//! ```
//!
//! and `main.rs` registers the plugin via
//! `tauri_plugin_updater::Builder::new().build()` (plus
//! `tauri_plugin_process::init()` so the JS side can call `relaunch()` after
//! `update.downloadAndInstall(...)` if it ever drives the install from the
//! renderer instead of the Rust command path).
//!
//! Wire-format reminders (see `deploy/UPDATES.md`):
//! * `tauri.conf.json` -> `bundle.createUpdaterArtifacts = true`
//! * `tauri.conf.json` -> `plugins.updater.pubkey` = contents of the public
//!   `.pub` file produced by `cargo tauri signer generate`
//! * `tauri.conf.json` -> `plugins.updater.endpoints` points at the
//!   `latest.json` we upload alongside each GitHub Release.

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

/// Renderer-facing summary of an available update. Mirrors the fields the
/// Phase 4 UI's "updates row" needs to render the prompt without a second
/// round-trip.
#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub published_at: String,
}

/// Ask the configured endpoint(s) whether an update is available. Returns
/// `None` if we are already on the latest version.
///
/// Errors are stringified at the boundary so the `#[tauri::command]` wrapper
/// can return them straight to the renderer without dragging the plugin's
/// concrete error type into Phase 4's command surface.
pub async fn check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let maybe_update = updater.check().await.map_err(|e| e.to_string())?;
    Ok(maybe_update.map(|u| UpdateInfo {
        version: u.version.clone(),
        notes: u.body.clone().unwrap_or_default(),
        published_at: u.date.map(|d| d.to_string()).unwrap_or_default(),
    }))
}

/// Download + install the pending update, then restart the app. No-op if
/// there is no update available at call time.
///
/// The progress / finished callbacks are intentionally no-ops: Phase 4's UI
/// just shows a spinner while the command is in flight. If we later want a
/// progress bar in the renderer, swap in an `app.emit("updater://progress", ..)`
/// pair.
pub async fn apply(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    if let Some(update) = updater.check().await.map_err(|e| e.to_string())? {
        update
            .download_and_install(|_chunk, _total| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
        app.restart();
    }
    Ok(())
}
