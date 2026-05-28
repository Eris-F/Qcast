//! Tauri shell entry point.
//!
//! Wires up:
//!   1. `AppState` (shared, mutable, lives for the process lifetime).
//!   2. The IPC command surface (`commands` module — one Rust fn per JS method).
//!   3. The system tray (`tray` module — three menu items + window-toggle).
//!   4. The mDNS browser (always running so the "Client" screen has live data).
//!   5. The global kill-hotkey (registered from Settings; re-registered on
//!      `update_settings`).
//!   6. A 500ms `lan_sessions_changed` push so the frontend doesn't have to poll.
//!
//! `windows_subsystem = "windows"` keeps the release build from popping a console.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod tray;
mod updater;

use std::sync::{Arc, Mutex, OnceLock};

use qcast_sender::{host::RunningHost, mdns::MdnsBrowser};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Process-wide shared state. All fields are independently lockable so a long
/// stop_share doesn't block a `get_settings` etc.
#[derive(Default)]
pub struct AppState {
    /// The running share host, if any. Locked briefly for take/replace.
    pub host: Arc<Mutex<Option<RunningHost>>>,
    /// The current session metadata mirror — populated alongside `host` so
    /// `current_share` can report the live access code + started_at without
    /// the UI re-asking after a window-hide round-trip.
    pub session: Mutex<Option<commands::ShareSession>>,
    /// The mDNS browser. `OnceLock` because we want a stable shared reference
    /// once `setup()` has initialized it.
    pub mdns: OnceLock<MdnsBrowser>,
    /// Persisted user settings (with sane defaults until the on-disk file is read).
    pub settings: Mutex<commands::Settings>,
}

fn main() {
    // tracing-subscriber init: env-controlled, mirrors the sender binary's pattern.
    // Failure here is non-fatal — tracing macros become no-ops if init didn't run.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("QCAST_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    tauri::Builder::default()
        // Phase 5 plugin registrations — wire the updater + process plugins so
        // the Settings screen's "Check for updates" path is real.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(AppState::default())
        .setup(|app| {
            // Load persisted settings before anything else: the kill-hotkey
            // registration below reads from this snapshot.
            let initial = commands::load_settings(app.handle());
            {
                let state = app.state::<AppState>();
                if let Ok(mut s) = state.settings.lock() {
                    *s = initial.clone();
                }
            }

            // Start the always-on mDNS browser. A failure here doesn't block
            // the app from starting — we just log and keep an empty browser
            // (the LAN list will stay empty until something else surfaces it).
            match MdnsBrowser::start() {
                Ok(browser) => {
                    let _ = app.state::<AppState>().mdns.set(browser);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "mDNS browser failed to start; LAN discovery disabled");
                }
            }

            // Register the kill-hotkey before the user can possibly invoke
            // `start_share`, so a panic-stop is available the instant streaming
            // starts. Failure logged + carried on; the UI still has its own
            // Stop button as a universal fallback.
            if let Err(e) = register_kill_hotkey(app.handle(), &initial.default_kill_hotkey) {
                tracing::warn!(error = %e, "could not register kill-hotkey");
            }

            tray::build(app.handle())?;

            // Background pusher: every 500ms emit `lan_sessions_changed` with
            // the current mDNS snapshot. The frontend can either listen to
            // this OR poll `list_lan_sessions`; this is the push-update bonus
            // that keeps the Client screen lively without UI-side timers.
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut last_signature: Option<String> = None;
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let Some(browser) = app_handle.state::<AppState>().mdns.get().cloned() else {
                        continue;
                    };
                    let snapshot: Vec<commands::LanSession> = browser
                        .snapshot()
                        .into_iter()
                        .map(commands::LanSession::from)
                        .collect();
                    // Only emit on change: a 500ms tick of empty-then-empty
                    // shouldn't wake the webview / re-render LAN list.
                    let sig = serde_json::to_string(&snapshot).unwrap_or_default();
                    if last_signature.as_deref() != Some(sig.as_str()) {
                        last_signature = Some(sig);
                        let _ = app_handle.emit("lan_sessions_changed", &snapshot);
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_share,
            commands::stop_share,
            commands::current_share,
            commands::connect_to_code,
            commands::connect_to_lan,
            commands::disconnect,
            commands::list_lan_sessions,
            commands::get_settings,
            commands::update_settings,
            commands::check_for_updates,
            commands::apply_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running qcast-app");
}

/// (Re-)register the kill-hotkey. Called once at setup and again whenever
/// `update_settings` changes `default_kill_hotkey`. Unregisters the previous
/// binding first so a rename doesn't leave a dangling stale shortcut.
pub fn register_kill_hotkey(app: &AppHandle, accelerator: &str) -> anyhow::Result<()> {
    let shortcut: Shortcut = accelerator
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid hotkey {accelerator:?}: {e}"))?;

    let gs = app.global_shortcut();
    // Best-effort: ignore "not registered" errors on the first call. The plugin
    // returns Err when there's nothing to unregister, which is fine here.
    let _ = gs.unregister_all();

    let app_clone = app.clone();
    gs.on_shortcut(shortcut, move |_app, _sc, _event| {
        // Don't await inside the callback (it's sync); spin the stop on the
        // async runtime so the hotkey returns immediately.
        let app_clone = app_clone.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = tray::stop_share_from_tray(&app_clone).await {
                tracing::warn!(error = %e, "kill-hotkey stop_share failed");
            }
        });
    })
    .map_err(|e| anyhow::anyhow!("register global shortcut: {e}"))?;

    tracing::info!(accelerator, "kill-hotkey registered");
    Ok(())
}
