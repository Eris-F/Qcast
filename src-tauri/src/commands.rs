//! Tauri IPC command surface — the bridge between the web-client UI and the
//! `qcast_sender` host library. Every command here corresponds 1:1 to a method
//! the TypeScript IPC stub (`web-client/src/lib/ipc.ts`) calls; the Rust names
//! are snake_case (which is also the JS invoke name).
//!
//! Boundary discipline:
//! - All shared state lives behind `AppState` and is locked with `std::sync::Mutex`
//!   for the short critical sections we have here (no async-mutex needed — we
//!   neither hold the lock across `.await` nor do long work under it).
//! - All errors bubble to JS as `Result<_, String>` with a user-facing message;
//!   detailed context is logged via `tracing` so the GUI doesn't have to.
//! - Settings persist to a JSON file under `app_config_dir()` — kept tiny so the
//!   file stays human-readable and a corrupt one is trivial to delete + recover.

use std::sync::Arc;

use qcast_sender::{access_code, host, mdns};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;

// ---------- DTOs (must mirror web-client/src/lib/ipc.ts exactly) -----------

/// The currently-running share session as the UI sees it. `started_at` is an
/// RFC3339-ish UTC timestamp so the UI can format it however it wants.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSession {
    pub code: String,
    pub started_at: String,
}

/// Options the user picks on the "Share" screen before starting a session.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareOptions {
    /// Global hotkey string (Tauri global-shortcut accelerator format,
    /// e.g. `"Ctrl+Alt+Q"`) that hard-stops the share.
    pub kill_hotkey: String,
    /// Whether the receiver may drive mouse/keyboard input on this host.
    /// (The host library always builds the input-injection probes; this flag is
    /// surfaced for future per-session gating + UI affordance.)
    pub allow_input: bool,
}

/// One LAN-discovered peer entry sent to the "Client" screen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSession {
    pub peer_id: String,
    pub display_name: String,
    pub addr: String,
    pub last_seen: u64,
}

impl From<mdns::LanSession> for LanSession {
    fn from(p: mdns::LanSession) -> Self {
        Self {
            peer_id: p.peer_id,
            display_name: p.display_name,
            addr: p.addr,
            // mdns::LanSession.last_seen is a std::time::Instant (monotonic, not
            // epoch-comparable). For the renderer we emit "milliseconds since the
            // session was last resolved" — adequate for the "recently seen"
            // sorting the Client screen actually does, and trivially serializable.
            last_seen: p.last_seen.elapsed().as_millis() as u64,
        }
    }
}

/// Persisted user preferences. Whole-document overwrites are fine; the file is
/// tiny and we never partial-write it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub default_kill_hotkey: String,
    pub auto_check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_kill_hotkey: "Ctrl+Alt+Q".to_string(),
            auto_check_updates: true,
        }
    }
}

/// Partial settings update from the UI. Only fields the user actually changed
/// are sent; the rest stay at their current values.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub default_kill_hotkey: Option<String>,
    pub auto_check_updates: Option<bool>,
}

// ---------- Commands -------------------------------------------------------

/// Start a share session: build the `qcast-sender` host, store it, hide the
/// main window into the tray, and emit `share_started` so the UI can react.
///
/// NOTE: `source = test` is hard-coded for now — real screen capture needs the
/// Wayland portal we can't trigger from a Tauri JS click on this Fedora host
/// anyway; the Windows build flips this to the d3d11/dxgi path in a follow-up.
#[tauri::command]
pub async fn start_share(
    app: AppHandle,
    state: State<'_, AppState>,
    opts: ShareOptions,
) -> Result<ShareSession, String> {
    {
        // Refuse a double-start: the host owns the global capture + ports, so a
        // second concurrent session would just fail at port-bind in a confusing
        // way. Surface a clear error instead.
        let guard = state.host.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("a share session is already running".to_string());
        }
    }

    let code = access_code::generate();
    let cfg = host::HostConfig {
        host: "0.0.0.0".to_string(),
        web_port: 8080,
        signalling_port: 8443,
        test_pattern: true,
        max_width: host::VIDEO_MAX_WIDTH,
        max_height: host::VIDEO_MAX_HEIGHT,
        codec_pref: host::CodecPref::default(),
        access_code: code.clone(),
    };

    // `host::start` may block until the portal/pipeline reaches `Playing`
    // (up to 120s on Linux with a real source). Run it on a blocking task so
    // we don't park a tokio worker.
    let running = tokio::task::spawn_blocking(move || host::start(cfg))
        .await
        .map_err(|e| format!("host spawn join error: {e}"))?
        .map_err(|e| format!("failed to start host: {e}"))?;

    tracing::info!(allow_input = opts.allow_input, hotkey = %opts.kill_hotkey, "share started");

    {
        let mut guard = state.host.lock().map_err(|e| e.to_string())?;
        *guard = Some(running);
    }

    let session = ShareSession {
        code,
        started_at: now_rfc3339(),
    };

    // Mirror the session into state so `current_share` can return the same
    // payload after a window-hide / show cycle without re-asking the host.
    if let Ok(mut s) = state.session.lock() {
        *s = Some(session.clone());
    }

    // Collapse window into the tray so the user's screen is unobstructed.
    // Failure here is non-fatal: streaming is up regardless of window state.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_skip_taskbar(true);
        let _ = w.hide();
    }

    // Tray menu reflects share state so right-click → "Stop sharing" lights up.
    crate::tray::refresh_menu_for_state(&app, true);

    // Push the session into the UI; the frontend can also call `current_share`
    // on demand if it missed the event (e.g. a late-attached devtools).
    let _ = app.emit("share_started", &session);
    Ok(session)
}

/// Stop the active share. Idempotent: stopping when nothing is running is a
/// success (the user's intent — "no share running" — is already true).
#[tauri::command]
pub async fn stop_share(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let taken = {
        let mut guard = state.host.lock().map_err(|e| e.to_string())?;
        guard.take()
    };

    if let Some(mut running) = taken {
        // The host's blocking `stop` joins the pipeline thread; offload it so
        // we don't park the tokio worker.
        tokio::task::spawn_blocking(move || running.stop())
            .await
            .map_err(|e| format!("host stop join error: {e}"))?;
        tracing::info!("share stopped");
    }

    // Drop the mirrored session metadata so `current_share` correctly reports
    // "nothing running" after stop completes.
    if let Ok(mut s) = state.session.lock() {
        *s = None;
    }

    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_skip_taskbar(false);
        let _ = w.show();
        let _ = w.set_focus();
    }

    crate::tray::refresh_menu_for_state(&app, false);

    let _ = app.emit("share_stopped", ());
    Ok(())
}

/// What's running right now (or `None`)? Used by the UI on mount to recover
/// state after a window-close/show cycle.
#[tauri::command]
pub fn current_share(state: State<'_, AppState>) -> Option<ShareSession> {
    state.session.lock().ok()?.clone()
}

/// PLACEHOLDER — the receiver implementation lives in a later phase. We accept
/// the call and acknowledge so the UI can wire the flow end-to-end.
#[tauri::command]
pub async fn connect_to_code(code: String) -> Result<(), String> {
    tracing::info!(?code, "connect_to_code (stub)");
    Ok(())
}

/// PLACEHOLDER — see `connect_to_code`. `rename_all = "camelCase"` so the JS
/// side can invoke with `{ peerId }` matching the rest of the IPC surface.
#[tauri::command(rename_all = "camelCase")]
pub async fn connect_to_lan(peer_id: String) -> Result<(), String> {
    tracing::info!(?peer_id, "connect_to_lan (stub)");
    Ok(())
}

/// PLACEHOLDER — see `connect_to_code`.
#[tauri::command]
pub async fn disconnect() -> Result<(), String> {
    tracing::info!("disconnect (stub)");
    Ok(())
}

/// LAN peers currently visible via mDNS. The mDNS browser runs continuously
/// from app startup; this is a cheap point-in-time read.
#[tauri::command]
pub fn list_lan_sessions(state: State<'_, AppState>) -> Vec<LanSession> {
    state
        .mdns
        .get()
        .map(|b| b.snapshot().into_iter().map(LanSession::from).collect())
        .unwrap_or_default()
}

/// Read the current persisted settings (or defaults if none persisted yet).
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state
        .settings
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

/// Patch the settings and persist them. Returns the FULL post-patch settings so
/// the UI doesn't have to call `get_settings` again. If the hotkey changed, the
/// caller re-registers the global shortcut (see `main.rs::register_kill_hotkey`).
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> Result<Settings, String> {
    let new_settings = {
        let mut guard = state.settings.lock().map_err(|e| e.to_string())?;
        if let Some(h) = patch.default_kill_hotkey.clone() {
            guard.default_kill_hotkey = h;
        }
        if let Some(b) = patch.auto_check_updates {
            guard.auto_check_updates = b;
        }
        guard.clone()
    };

    persist_settings(&app, &new_settings)
        .map_err(|e| format!("could not persist settings: {e}"))?;

    // If the hotkey changed, re-register it so the new accelerator is live
    // immediately (no app-restart required).
    if patch.default_kill_hotkey.is_some() {
        if let Err(e) = crate::register_kill_hotkey(&app, &new_settings.default_kill_hotkey) {
            tracing::warn!(error = %e, "failed to re-register kill hotkey");
        }
    }

    let _ = app.emit("settings_changed", &new_settings);
    Ok(new_settings)
}

/// Ask the configured endpoint(s) whether an update is available.
/// Wired to `crate::updater::check` from the Phase 5 module.
#[tauri::command]
pub async fn check_for_updates(
    app: AppHandle,
) -> Result<Option<crate::updater::UpdateInfo>, String> {
    crate::updater::check(app).await
}

/// Download + install the pending update, then restart the app.
/// Wired to `crate::updater::apply` from the Phase 5 module.
#[tauri::command]
pub async fn apply_update(app: AppHandle) -> Result<(), String> {
    crate::updater::apply(app).await
}

// ---------- Helpers --------------------------------------------------------

/// Where the settings JSON lives. Anchored at Tauri's per-user config dir so
/// every OS lands in the right place (e.g. `%APPDATA%/app.qcast.desktop/` on
/// Windows, `~/.config/app.qcast.desktop/` on Linux).
fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("resolve app_config_dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

/// Load settings from disk, or return defaults if the file is missing or
/// unreadable. Bad-JSON tolerance is intentional: a corrupt file should never
/// brick the app; the user can fix or delete it and the defaults take over.
pub fn load_settings(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else {
        return Settings::default();
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

/// Persist settings. Creates the parent dir on first write.
fn persist_settings(app: &AppHandle, s: &Settings) -> anyhow::Result<()> {
    let path = settings_path(app).map_err(|e| anyhow::anyhow!(e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(s)?;
    std::fs::write(&path, body)?;
    Ok(())
}

/// Best-effort UTC timestamp formatted as RFC3339-ish for the UI. We avoid
/// pulling `chrono` for one timestamp; the format is `YYYY-MM-DDTHH:MM:SSZ`.
fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Cheap UTC breakdown via standard arithmetic (date-only granularity day-
    // accurate from the unix epoch). For sub-second precision a `chrono`-style
    // dep is overkill; the UI only needs "wall-clock at start".
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Civil date+time from unix seconds (UTC). Standard "days from civil" trick;
/// keeps us free of chrono for the one timestamp string we render to JS.
fn unix_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let day = (secs / 86_400) as i64;
    let sod = (secs % 86_400) as u32;
    let h = sod / 3600;
    let mi = (sod % 3600) / 60;
    let s = sod % 60;
    // Civil-from-days (Howard Hinnant) starting from 1970-01-01 (day 0).
    let z = day + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y };
    (y as i32, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Defaults are stable and sane: hotkey + auto-update on.
    #[test]
    fn settings_default_is_sane() {
        let s = Settings::default();
        assert_eq!(s.default_kill_hotkey, "Ctrl+Alt+Q");
        assert!(s.auto_check_updates);
    }

    /// Round-trip a known unix timestamp through the date breakdown so a typo
    /// in the formula doesn't silently drift the wall clock we show.
    #[test]
    fn unix_to_ymd_hms_known_value() {
        // 2024-01-01T00:00:00Z = 1_704_067_200
        let (y, mo, d, h, mi, s) = unix_to_ymd_hms(1_704_067_200);
        assert_eq!((y, mo, d, h, mi, s), (2024, 1, 1, 0, 0, 0));
    }

    /// Patch logic merges field-by-field (no destructive overwrites of fields
    /// the user didn't touch).
    #[test]
    fn patch_merges_only_set_fields() {
        let mut s = Settings::default();
        let p = SettingsPatch {
            default_kill_hotkey: Some("Ctrl+Shift+X".into()),
            auto_check_updates: None,
        };
        if let Some(h) = p.default_kill_hotkey {
            s.default_kill_hotkey = h;
        }
        if let Some(b) = p.auto_check_updates {
            s.auto_check_updates = b;
        }
        assert_eq!(s.default_kill_hotkey, "Ctrl+Shift+X");
        assert!(s.auto_check_updates, "untouched field must stay default");
    }
}

// Re-exported so `main.rs` can pass it into `app.manage(...)` without naming
// the host crate type directly in two places.
pub type SharedHost = Arc<std::sync::Mutex<Option<host::RunningHost>>>;
