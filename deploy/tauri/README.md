# Qcast — Tauri app scaffold (build on Windows)

> **Status:** blueprint. Tauri **cannot be built on the Fedora dev box** (it needs
> `webkit2gtk4.1-devel` + `gtk3-devel` + `cargo-tauri`, none installed, and we have no
> passwordless sudo). So this directory is a **copy-pasteable scaffold** to create the
> app on Windows (the only target for now). Everything here encodes decisions already
> made in `deploy/WINDOWS_INSTALLER.md` (NSIS + WebView2 + GStreamer-as-resources) and
> the remote-support pivot, so building it is mechanical.

This is the **Path A** receiver/sender shell: video is consumed **inside WebView2**
(Chromium → solid WebRTC), reusing the existing `crates/qcast-sender/web-client/`. One
binary, **role chosen at launch** (`share` vs `connect`).

---

## 0. Prerequisites

**Windows build box** (same as `deploy/windows/README.md` plus Tauri):
- VS 2022 Build Tools — Desktop C++ workload
- GStreamer 1.26 MSVC x86_64 — runtime **and** devel MSIs, `ADDLOCAL=ALL`
- Rust MSVC (`x86_64-pc-windows-msvc`) + `cargo-c`
- `cargo install tauri-cli --version "^2"` (provides `cargo tauri`)
- WebView2 is bundled by the installer (`offlineInstaller`), not a build dep

**Linux dev box** (only if you want `cargo tauri dev` on Fedora later — optional, video
in WebKitGTK is the shaky case the pivot avoids): `sudo dnf install webkit2gtk4.1-devel
gtk3-devel libsoup3-devel` + `cargo install tauri-cli`.

## 1. Create the crate

```bash
# from repo root, on the build box
cargo tauri init        # or: cargo create-tauri-app
#   App name: Qcast
#   frontendDist: ../../crates/qcast-sender/web-client   (reuse the existing client)
#   no framework / no bundler (the client is static)
```

Put the crate at `src-tauri/` (Tauri default). **Keep it OUT of the root cargo
workspace** (add `"src-tauri"` to `exclude` in the root `Cargo.toml`, or give it its own
`[workspace]`) so the Fedora `cargo build --workspace` stays green without the WebKitGTK
deps. Then replace the generated `tauri.conf.json` with the template in this directory.

## 2. `tauri.conf.json` (this dir)

The template here already sets:
- `build.frontendDist` → the existing `web-client/` (the gstwebrtc-api consumer, now
  also the remote-control **receiver**).
- `bundle.targets: ["nsis"]`, `windows.nsis.installMode: "currentUser"` — per-user,
  **no UAC**, installs to `%LOCALAPPDATA%`.
- `windows.webviewInstallMode: offlineInstaller` — offline-capable, evergreen WebView2.
- `bundle.resources` — the bundled GStreamer plugins + scanner (staged by
  `deploy/windows/gather-payload.ps1` into a `gst-runtime/` dir next to `tauri.conf.json`
  before `cargo tauri build`). These land under `…\resources\` on install; `bundle.rs`
  already has the `resources\lib\gstreamer-1.0` candidate path.
- `app.trayIcon` + `app.windows[0]` — base config; tray behavior + branding are applied
  at runtime (§4).

**Risk to settle on Windows (from research):** the *flat* top-level GStreamer DLLs
(`gstreamer-1.0-0.dll`, glib, …) must be on the exe's DLL search path. `resources\` is a
subfolder Windows won't search by default. Mitigate with an `nsis.installerHooks`
`.nsh` (`NSIS_HOOK_POSTINSTALL`) that copies the flat DLLs from `resources\` up to
`$INSTDIR`, **or** call `SetDllDirectoryW`/`AddDllDirectory` at startup, **or** fall
back to the Inno flat layout (`deploy/windows/`, Prototype B). See WINDOWS_INSTALLER.md §8.

## 3. Role at launch (one binary: `share` | `connect`)

`src-tauri/src/main.rs` parses the first arg:

```rust
// share  → run the GStreamer capture pipeline + the SendInput injector (controlled side)
// connect→ open a WebView2 window that loads the receiver client (controller side)
enum Role { Share, Connect }

fn role_from_args() -> Role {
    match std::env::args().nth(1).as_deref() {
        Some("connect") => Role::Connect,
        _ => Role::Share, // default
    }
}
```

- **`share`** reuses the sender pipeline. Refactor `crates/qcast-sender`'s `host` /
  `turn` / `capture` / `input` modules into a small **library** (e.g. promote them to
  `qcast-core` or a new `qcast-host` lib crate) the Tauri app calls, instead of
  duplicating. `bundle::configure_plugin_path()` must run **before** `gst::init()` — call
  it (or the `resource_dir()`-based variant) first thing in the Tauri `setup`.
- **`connect`** points the WebView2 window at the receiver client (already remote-control
  capable via `attachVideoElement`).

## 4. Tray (not taskbar) + configurable name / icon / title  *(the requested feature)*

All applied in the Tauri `setup` hook, overridable by startup flags
(`--name <s>`, `--icon <path>`, `--title <s>`, `--tray`):

```rust
use tauri::{
    tray::TrayIconBuilder,
    menu::{Menu, MenuItem},
    Manager, WindowEvent,
};

tauri::Builder::default()
    .setup(|app| {
        let win = app.get_webview_window("main").unwrap();

        // --- Configurable branding (startup flags / config) ---
        if let Some(title) = cli_flag("--title").or_else(|| cli_flag("--name")) {
            win.set_title(&title)?;
        }

        // --- Tray with restore + a one-click "Stop sharing" (the load-bearing stop) ---
        let show = MenuItem::with_id(app, "show", "Show Qcast", true, None::<&str>)?;
        let stop = MenuItem::with_id(app, "stop", "Stop sharing", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &stop, &quit])?;

        let mut tray = TrayIconBuilder::with_id("qcast-tray")
            .menu(&menu)
            .tooltip(cli_flag("--name").as_deref().unwrap_or("Qcast"))
            .on_menu_event(|app, e| match e.id().as_ref() {
                "show" => { let _ = app.get_webview_window("main").map(|w| { let _ = w.show(); let _ = w.set_focus(); }); }
                "stop" => { /* halt capture + any in-flight injection at once, then hide */ }
                "quit" => app.exit(0),
                _ => {}
            });
        if let Some(icon_path) = cli_flag("--icon") { tray = tray.icon(tauri::image::Image::from_path(icon_path)?); }
        tray.build(app)?;

        // --- Minimize to tray, NOT the taskbar ---
        // When tray mode is on, closing/minimizing hides the window + removes its
        // taskbar button; the tray icon is the way back.
        if cli_present("--tray") {
            let w = win.clone();
            win.on_window_event(move |ev| {
                if let WindowEvent::CloseRequested { api, .. } = ev {
                    api.prevent_close();
                    let _ = w.hide();
                    let _ = w.set_skip_taskbar(true);
                }
            });
        }
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("run qcast");
```

Notes / boundaries:
- **`set_skip_taskbar(true)` + hide-to-tray** is the "minimize to tray, not taskbar"
  behavior. The window comes back via the tray "Show" item.
- **Configurable name/icon/title** = the tray tooltip, tray icon image, and window
  title set from flags at startup. The OS *process/exe* name is the installed exe's
  filename (set by `productName` / the installer) — rename there if needed.
- This is **branding/presence**, consistent with the consent model (the controlled
  friend still knows it runs + can stop it via the tray). It is **not** process
  concealment — keep the "Stop sharing" affordance and an identifiable presence.
- `Cargo.toml` features: `tauri = { version = "2", features = ["tray-icon", "image-png"] }`.

## 5. Build the installer — one command

After creating the app (§1) and copying this dir's `tauri.conf.json` +
`installer-hooks.nsh` into it:

```powershell
deploy\tauri\build-windows.ps1
#  1. stages GStreamer into <TauriDir>\gst-runtime\{bin,lib,libexec}
#  2. cargo tauri build
#  → src-tauri\target\release\bundle\nsis\Qcast_<ver>_x64-setup.exe
```

`installer-hooks.nsh` (wired via `nsis.installerHooks`) relocates the flat
`gst-runtime\bin` DLLs next to the exe at install time — the fix for risk #1
(plugins + scanner stay under `resources\`, where `bundle.rs` finds them).

Validate with `deploy/WINDOWS_INSTALLER.md` §9 and `deploy/TEST_PLAN.md` (Layers 3–5).

## 6. What's deferred

- The `qcast-sender` → library refactor for the `share` role (do it on the build box so
  you can compile-check the WebKitGTK/WebView2 paths).
- Receiver-side input forwarding is already wired in `web-client/app.js`
  (`attachVideoElement`) — validate it end-to-end in WebView2.
- Pairing secret at the signalling layer (mandatory once input is injected) — tracked
  separately; the current short-code gate is client-side only.
