//! System-tray menu wiring.
//!
//! Three items: "Stop sharing" (toggled by whether a share is live), "Show
//! Qcast" (un-hides the main window the share collapsed away), and "Quit"
//! (clean exit — stops the share first if one is running).
//!
//! All actions emit Tauri events to the frontend so the UI can update its
//! state model, and they ALSO directly drive the window/state so the tray
//! works even before the frontend is mounted (e.g. right after launch).

use std::sync::OnceLock;

use tauri::{
    menu::{Menu, MenuBuilder, MenuItemBuilder},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, Wry,
};

use crate::AppState;

/// The tray menu, stashed at build time so [`refresh_menu_for_state`] can toggle
/// the "Stop sharing" item without round-tripping through the tray icon. Tauri 2
/// doesn't expose a `TrayIcon::menu()` getter, so we keep our own reference.
static TRAY_MENU: OnceLock<Menu<Wry>> = OnceLock::new();

/// IDs we set on menu items so click handlers can match without string-equality
/// against (translated) labels.
const ID_STOP: &str = "tray-stop-share";
const ID_SHOW: &str = "tray-show";
const ID_QUIT: &str = "tray-quit";

/// Build the tray + menu and register click handlers. Called from `main`'s
/// `setup` hook (so the `AppHandle` is fully initialized).
pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    // Stash a clone so `refresh_menu_for_state` can mutate items later.
    let _ = TRAY_MENU.set(menu.clone());

    let mut builder = TrayIconBuilder::with_id("qcast-tray")
        .tooltip("Qcast")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_icon_event);

    // Reuse the bundled window icon (icons/icon.png — already in tauri.conf.json's
    // bundle list) so we don't have to ship a second tray-specific asset.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let _tray = builder.build(app)?;

    Ok(())
}

/// Construct the menu. "Stop sharing" starts disabled — `refresh_menu_for_state`
/// flips it once a share is live (and back when it stops).
fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let stop = MenuItemBuilder::with_id(ID_STOP, "Stop sharing")
        .enabled(false)
        .build(app)?;
    let show = MenuItemBuilder::with_id(ID_SHOW, "Show Qcast").build(app)?;
    let quit = MenuItemBuilder::with_id(ID_QUIT, "Quit").build(app)?;

    MenuBuilder::new(app)
        .item(&stop)
        .separator()
        .item(&show)
        .separator()
        .item(&quit)
        .build()
}

/// Tray-menu click router. The work here is intentionally small + synchronous;
/// any blocking work (stopping the host) gets offloaded to a blocking task so
/// the tray click feels instant.
fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        ID_STOP => {
            // Mirror what the `stop_share` command does, but invoked directly
            // from the tray so it works without a frontend round-trip.
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = stop_share_from_tray(&app_clone).await {
                    tracing::warn!(error = %e, "tray stop_share failed");
                }
            });
        }
        ID_SHOW => {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_skip_taskbar(false);
                let _ = w.show();
                let _ = w.set_focus();
            }
            let _ = app.emit("tray_show", ());
        }
        ID_QUIT => {
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                // Best-effort: stop a running share before exit so the
                // capture/portal handle is released cleanly.
                let _ = stop_share_from_tray(&app_clone).await;
                app_clone.exit(0);
            });
        }
        _ => {}
    }
}

/// Click on the tray icon itself (not the menu). Left-click un-hides the
/// window — the menu is opened on right-click by the platform.
fn handle_tray_icon_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: tauri::tray::MouseButton::Left,
        button_state: tauri::tray::MouseButtonState::Up,
        ..
    } = event
    {
        let app = tray.app_handle();
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_skip_taskbar(false);
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
}

/// Shared tray + global-hotkey stop path. Drains the host, restores the
/// window, emits `share_stopped`, and refreshes the menu enabled state.
pub async fn stop_share_from_tray(app: &AppHandle) -> Result<(), String> {
    let taken = {
        let state = app.state::<AppState>();
        let mut guard = state.host.lock().map_err(|e| e.to_string())?;
        guard.take()
    };
    if let Some(mut running) = taken {
        tokio::task::spawn_blocking(move || running.stop())
            .await
            .map_err(|e| format!("host stop join error: {e}"))?;
        tracing::info!("share stopped (from tray)");
    }
    // Mirror state cleanup so `current_share` is correct after a tray-stop.
    if let Ok(mut s) = app.state::<AppState>().session.lock() {
        *s = None;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_skip_taskbar(false);
        let _ = w.show();
        let _ = w.set_focus();
    }
    let _ = app.emit("share_stopped", ());
    refresh_menu_for_state(app, false);
    Ok(())
}

/// Toggle the "Stop sharing" menu item to match share state. Called by
/// `start_share` / `stop_share` paths so the tray UX stays in sync.
pub fn refresh_menu_for_state(_app: &AppHandle, sharing: bool) {
    let Some(menu) = TRAY_MENU.get() else {
        return;
    };
    if let Some(kind) = menu.get(ID_STOP) {
        if let Some(item) = kind.as_menuitem() {
            let _ = item.set_enabled(sharing);
        }
    }
}
