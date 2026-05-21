# Qcast packaging plan — Linux AppImage + Windows bundled installer

Goal: ship Qcast as **prebuilt, drag-and-drop artifacts** so end users never install a
toolchain or compile anything. Linux → a single **AppImage**; Windows → a single **bundled
installer** (`.exe` via Inno Setup first; `.msi` via WiX optional later). The existing
`deploy/setup-linux.sh` / `setup-windows.ps1` remain the **contributor / build-machine** path.

This file is the durable spec — safe to resume from after a context compaction.

---

## 0. Current state (what's already true)

- Working, validated on Linux (Fedora 43 / GStreamer 1.26): capture → `videoscale`(1080p cap)
  → `webrtcsink` (VP8-preferred, H.264 fallback), **in-process TURN relay** (`turn` crate, no
  coturn), forced ICE relay, pre-launch egui GUI that closes to a background process, stop via
  Ctrl+Alt+Q / kill. `--headless` path exists for tests.
- `webrtcsink` comes from gst-plugins-rs (branch `0.15`), built with `cargo cbuild
  -p gst-plugin-webrtc` → `libgstrswebrtc.so` / `gstrswebrtc.dll`.
- Web client lives in `crates/qcast-sender/web-client/` and is referenced at runtime by
  `WEB_CLIENT_DIR = concat!(env!("CARGO_MANIFEST_DIR"), "/web-client")` — an **absolute
  compile-time path**.

---

## 1. PREREQUISITE code change — make the binary relocatable (blocks ALL bundling)

Two things assume the build tree exists at runtime. Both must be fixed before any bundle works
on a machine that doesn't have the repo:

### 1a. Embed the web client into the binary
`WEB_CLIENT_DIR` points at a path under the source tree — it won't exist for an end user.
- **Fix (recommended):** embed `web-client/` with the `include_dir` crate
  (`include_dir!("$CARGO_MANIFEST_DIR/web-client")`); at startup, extract it to a runtime dir
  (e.g. `$XDG_CACHE_HOME/qcast/web-client` or a temp dir) and pass *that* path to
  `webrtcsink web-server-directory=`.
- Keeps the binary self-contained (one file works for AppImage and the Windows bundle).
- Alternative: ship the dir next to the exe and resolve via `std::env::current_exe()`. Rejected —
  less robust, more moving parts.

### 1b. Find the bundled GStreamer plugins relative to the exe
A bundled app must locate `webrtcsink` (and friends) without a system GStreamer install.
- **Fix:** before `gst::init()`, prepend exe-relative plugin dirs to `GST_PLUGIN_PATH`
  (and set `GST_PLUGIN_SYSTEM_PATH_1_0` empty in the AppImage so it doesn't pick host plugins of
  a mismatched version). Helper: `current_exe()` → resolve `../lib/gstreamer-1.0` (Linux) /
  `.\` + `lib\gstreamer-1.0` (Windows) and `set_var` before init. Also set `GST_PLUGIN_SCANNER`
  to the bundled scanner.
- Make it a no-op when running from a normal dev build (so `cargo run` still uses the system /
  user plugin dir). Gate on "are we inside a bundle" (e.g. an env var the AppRun/installer sets,
  or detect a sibling `lib/gstreamer-1.0`).

**Validation for step 1:** copy `target/release/qcast-sender` to an empty dir *outside* the repo
and run it; it must still serve the web client and find webrtcsink (with the bundled plugin path
wired). This proves relocatability before we wrap it.

---

## 2. Linux AppImage

**Outcome:** `Qcast-x86_64.AppImage` — one executable file, no system deps, runs across distros.

**Tooling:** `linuxdeploy` + `linuxdeploy-plugin-gstreamer` (purpose-built to bundle the
GStreamer libs + plugins + the plugin scanner and wire the env).

**Steps**
1. `cargo build --release -p qcast-sender`.
2. Build the webrtc plugin (`cargo cbuild --release -p gst-plugin-webrtc`) → copy
   `libgstrswebrtc.so` into the AppDir's `usr/lib/gstreamer-1.0/`.
3. Lay out the AppDir:
   - `usr/bin/qcast-sender`
   - `usr/share/applications/qcast.desktop` (+ `usr/share/icons/.../qcast.png`) — AppImage
     requires a desktop entry + icon.
4. Run `linuxdeploy --appdir AppDir -e usr/bin/qcast-sender --plugin gstreamer -d qcast.desktop -i qcast.png --output appimage`.
   - The gstreamer plugin pulls in the needed GStreamer libs + plugins (coreelements,
     videoconvertscale, vpx, rtp/rtpmanager, nice, dtls, srtp, sctp, pipewire) and sets
     `GST_PLUGIN_PATH` / `GST_PLUGIN_SCANNER` in the generated AppRun.
   - Confirm our `libgstrswebrtc.so` and its Rust deps land in the bundle.
5. Output + optionally GPG-sign the AppImage.

**Gotchas / decisions**
- **Do NOT bundle** libGL/mesa/driver libs or glibc — use the host's (linuxdeploy excludelist
  handles most). Bundling Mesa breaks GPU/driver matching for eframe/wgpu and VAAPI.
- `pipewiresrc` + the xdg-desktop-portal are **host runtime services**, not bundled — present on
  any modern desktop; the AppImage just uses them. (X11 fallback `ximagesrc` needs the host X
  libs, also present.)
- Verify the bundled set actually contains: `webrtcsink`, `videoconvert`, `videoscale`,
  `vp8enc`, `rtpbin`, `nicesink`, `dtlsenc`, `srtpenc`, `pipewiresrc`. Run
  `gst-inspect-1.0` *from inside* the AppImage env to confirm.
- The in-process TURN relay needs no bundling (it's Rust, in the binary).
- egui/wgpu: ships its own Rust code; relies on host Vulkan/GL — fine.

**Test:** run on the dev box, then on a *different* distro or a clean container (e.g. an Ubuntu
docker/VM with a desktop session) to prove portability.

---

## 3. Windows bundled installer (`.exe` first, `.msi` optional)

**Outcome:** `Qcast-Setup.exe` — installs the prebuilt binary + the GStreamer runtime DLLs +
plugins + our plugin; Start-Menu shortcut; **no winget / Rust / MSVC / compile** on the user's
machine.

**Build the payload first** (on a Windows build machine or a CI windows runner — can't be done
from the Linux dev box):
1. `cargo build --release -p qcast-sender` (MSVC) → `qcast-sender.exe`.
2. `cargo cbuild --release -p gst-plugin-webrtc` → `gstrswebrtc.dll`.
3. Gather the GStreamer **runtime** payload from the installed MSVC runtime:
   - `bin\*.dll` (the runtime libs — start by bundling the full runtime `bin`; trim later with
     a dependency tracer like `Dependencies.exe`/`dumpbin /dependents` if size matters).
   - `lib\gstreamer-1.0\*.dll` plugins needed: coreelements, videoconvertscale, vpx, rtp,
     rtpmanager, webrtc deps (nice, dtls, srtp, sctpenc/dec), d3d11 (capture), plus our
     `gstrswebrtc.dll`.
   - `libexec\gstreamer-1.0\gst-plugin-scanner.exe`.

**Installer (Inno Setup `.iss`)**
- `[Setup]`: `AppName=Qcast`, version, `ArchitecturesAllowed=x64`, **per-user install** to
  `{localappdata}\Programs\Qcast` with `PrivilegesRequired=lowest` — avoids the admin/UAC prompt
  for a smoother first run (revisit if a system-wide install is wanted).
- `[Files]`: `qcast-sender.exe` → app dir; GStreamer `bin\*.dll` → app dir; plugins →
  `{app}\lib\gstreamer-1.0`; scanner → `{app}\libexec\gstreamer-1.0`; web client only if NOT
  embedded (step 1a makes it unnecessary).
- `[Icons]`: Start-Menu shortcut → `{app}\qcast-sender.exe`.
- Plugin path: rely on step 1b (the app sets `GST_PLUGIN_PATH` relative to its exe at startup) —
  do **not** require the user to set env vars.
- d3d11 capture is in the bundled `-bad` plugins; no portal/relay needed (TURN is in-binary).

**Code signing:** Authenticode-sign `qcast-sender.exe` + `Qcast-Setup.exe` to avoid SmartScreen
"unknown publisher" warnings (needs a cert — note as a real step toward "no scary first run").

**`.msi` (WiX) — optional later:** better for enterprise/Group-Policy deployment; harder to
author than Inno. Defer unless needed.

**Test:** on a clean Windows 10/11 VM with nothing installed — run the installer, launch from the
Start Menu, confirm the GUI + a phone can view the stream.

---

## 4. CI release pipeline (underpins reproducible artifacts)

GitHub Actions, on tag / release:
- **linux job:** build binary + plugin → assemble AppDir → AppImage → upload to the Release.
- **windows job:** build binary + plugin (MSVC) → gather GStreamer runtime → Inno → upload.
- (later) macOS job → `.app` + GStreamer.framework.
This replaces "build on the user's machine" with "download a prebuilt artifact" — the core win.

---

## 5. Sequencing

1. **Step 1 (relocatable binary)** — embed web client + exe-relative plugin path. Test by
   running a copied binary outside the repo. *This unblocks everything.*
2. **Linux AppImage** — buildable + testable here. Produce, verify elements inside the bundle,
   test on a second distro/container.
3. **Windows installer** — needs a Windows machine/CI; build payload, write the Inno `.iss`,
   produce + test on a clean Windows VM.
4. **CI pipeline** to automate 2 + 3.
5. **Signing** (Windows Authenticode, AppImage GPG; macOS notarization when macOS is added).

## Licensing note
We prefer **VP8** (royalty-free), so bundling avoids the H.264/openh264 (Cisco binary) licensing
wrinkle. GStreamer core/base/good are LGPL — dynamic bundling is fine. Keep `-ugly`/`-bad`
inclusions minimal and check any codec we actually ship.
