# Qcast packaging plan — Linux AppImage + Windows bundled installer

Goal: ship Qcast as **prebuilt, drag-and-drop artifacts** so end users never install a
toolchain or compile anything. Linux → a single **AppImage**; Windows → a single **bundled
installer** (`.exe` via Inno Setup first; `.msi` via WiX optional later). The existing
`deploy/setup-linux.sh` / `setup-windows.ps1` remain the **contributor / build-machine** path.

This file is the durable spec — safe to resume from after a context compaction.

**Status at a glance** (details per section):

| § | Deliverable | Status |
|---|---|---|
| 1 | Relocatable binary (embed web client + exe-relative plugin path) | **DONE** |
| 2 | Linux AppImage | **BUILT + validated on Fedora 43** (core-libs bundling + cross-distro test are follow-ups) |
| 3 | Windows bundled installer (Inno Setup) | **Recipe authored, untested on Windows** |
| 4 | CI release pipeline | **Drafted, not yet run on GitHub Actions** |

---

## 0. Current state (what's already true)

- Working, validated on Linux (Fedora 43 / GStreamer 1.26), incl. real WebRTC playback to
  devices: capture → `videoscale`(1080p cap, configurable) → `webrtcsink` (VP8-preferred,
  H.264 fallback; codec preference selectable), **in-process TURN relay** (`turn` crate, no
  coturn), forced ICE relay, **self-supervising pipeline** (bounded auto-restart on
  error/EOS, reuses the captured source so it never re-pops the picker), pre-launch egui GUI
  (resolution + codec + viewer-password options) that closes to a background process, stop via
  Ctrl+Alt+Q / kill. A **client-side viewer-password gate** runs in the browser (UX gate, not
  signalling-enforced auth — future work). `--headless` path exists for tests.
- `webrtcsink` comes from gst-plugins-rs (branch `0.15`), built with `cargo cbuild
  -p gst-plugin-webrtc` → `libgstrswebrtc.so` / `gstrswebrtc.dll`. Congestion control
  (`rtpgccbwe`) ships in the gst-plugins-rs RTP plugin (`libgstrsrtp.so`).
- The web client (`crates/qcast-sender/web-client/`) is now **embedded into the binary** with
  `include_dir` and extracted to a runtime dir at startup (see §1); `QCAST_WEB_CLIENT_DIR`
  overrides it for live web-client development.

---

## 1. PREREQUISITE code change — make the binary relocatable (blocks ALL bundling)

> **STATUS: DONE.** Both halves are implemented (`crates/qcast-sender/src/host.rs` for the
> embedded web client, `crates/qcast-sender/src/bundle.rs` for the exe-relative plugin path).
> A copied binary runs outside the build tree.

Two things assumed the build tree existed at runtime. Both are now fixed so a bundle works on a
machine that doesn't have the repo:

### 1a. Embed the web client into the binary — **done**
The old `WEB_CLIENT_DIR` pointed at a path under the source tree — it wouldn't exist for an end
user.
- **Implemented:** `web-client/` is embedded with `include_dir`
  (`include_dir!("$CARGO_MANIFEST_DIR/web-client")`) in `host.rs`; at startup it's extracted to
  a fresh per-process temp dir (`qcast-web-<pid>`, cleaned up on shutdown) and *that* path is
  passed to `webrtcsink web-server-directory=`. `session.json` (the access code) is written
  there before the pipeline starts.
- The binary is self-contained — one file works for the AppImage and the Windows bundle.
- **Dev override:** `QCAST_WEB_CLIENT_DIR=<dir>` serves the client straight from that dir (no
  extraction), so the web client can be edited live without recompiling.

### 1b. Find the bundled GStreamer plugins relative to the exe — **done**
A bundled app must locate `webrtcsink` (and friends) without a system GStreamer install.
- **Implemented (`bundle.rs::configure_plugin_path`, called before `gst::init()`):** prepends
  exe-relative plugin dirs to `GST_PLUGIN_PATH` (candidates, in order: `../lib/gstreamer-1.0`
  for the AppImage `usr/bin`→`usr/lib` layout, `./lib/gstreamer-1.0` for the Windows app dir,
  `./gstreamer-1.0` flat). It also points `GST_PLUGIN_SCANNER` at a bundled scanner if present.
- It is a **strict no-op** when no sibling bundled plugin dir exists, so a normal `cargo run`
  keeps using the system / user plugin dir untouched.
- It clears `GST_PLUGIN_SYSTEM_PATH_1_0` (so a mismatched host GStreamer can't shadow the
  bundled plugins) **only when `QCAST_BUNDLE=1`**. The AppRun sets this; on Windows a Start-Menu
  shortcut can't set env vars, so see `deploy/windows/README.md` for the open item there.

**Validation for step 1 (done):** a copied `target/release/qcast-sender` run from an empty dir
outside the repo still serves the web client and finds webrtcsink.

---

## 2. Linux AppImage

> **STATUS: BUILT + validated on Fedora 43.** `deploy/appimage/build-appimage.sh` produces a
> working `Qcast-x86_64.AppImage` that runs on a typical desktop and serves real WebRTC.
> A required fix landed during bring-up: the **webrtc (`webrtcbin`) plugin and `rtpgccbwe`
> congestion control** must be bundled alongside our `libgstrswebrtc.so`, or webrtcsink fails
> at runtime. **Honest follow-ups (not yet done):** (a) it still relies on the host's GStreamer
> **core** runtime libraries — present on typical desktops, but full core-lib bundling is
> tracked; (b) **cross-distro portability is unverified** — built and tested only on Fedora 43.

**Outcome:** `Qcast-x86_64.AppImage` — one executable file, runs on a typical Linux desktop.

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
- **Bundle the webrtc plugin + congestion control (fixed).** Beyond our `libgstrswebrtc.so`,
  the bundle MUST include the core **`webrtc`** plugin (`webrtcbin`, which webrtcsink drives)
  and **`rtpgccbwe`** (Google Congestion Control, in the gst-plugins-rs RTP plugin
  `libgstrsrtp.so`). Missing either makes webrtcsink fail at runtime in the AppImage even
  though it works against the dev install. `build-appimage.sh` stages both explicitly.
- **Core runtime libs (follow-up).** The current AppImage relies on the host's GStreamer
  **core** runtime libraries (the typical-desktop assumption). Fully bundling them so it's
  self-contained even on a bare system is a tracked follow-up.
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

**Test:** validated on the dev box (Fedora 43). **Still to do:** run on a *different* distro or
a clean container (e.g. an Ubuntu docker/VM with a desktop session) to prove portability —
this is the open cross-distro item. The CI Linux job (see §4 / `deploy/CI.md`) runs the script
on `ubuntu-latest` and is one way to shake this out.

---

## 3. Windows bundled installer (`.exe` first, `.msi` optional)

> **STATUS: recipe authored, NOT yet built or tested on Windows.** The Inno Setup recipe lives
> in `deploy/windows/` (`gather-payload.ps1` + `qcast.iss` + `README.md`); it was written on
> Linux and the GStreamer MSVC payload can only be assembled on a Windows machine, so the
> first run there is the validation. See `deploy/windows/README.md` for the build sequence,
> the clean-VM test plan, and the open items (e.g. `QCAST_BUNDLE=1` and the Start-Menu
> shortcut, runtime-`bin` trimming, plugin-DLL-name verification).

**Outcome:** `Qcast-Setup-<version>.exe` — installs the prebuilt binary + the GStreamer runtime
DLLs + plugins + our plugin; Start-Menu shortcut; **no winget / Rust / MSVC / compile** on the
user's machine.

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

> **STATUS: DRAFTED, not yet validated.** The workflow exists at
> `.github/workflows/release.yml` but has **not been run on GitHub Actions** — treat the first
> run (ideally a `workflow_dispatch`, not a tag) as the validation pass and expect to iterate.
> Full details, per-job inputs, and the known first-run risks are in **`deploy/CI.md`**.

GitHub Actions, on a `v*` tag (attaches artifacts to the Release) or `workflow_dispatch`
(uploads build artifacts only):
- **`linux-appimage` job (`ubuntu-latest`):** build binary + plugins → run
  `deploy/appimage/build-appimage.sh` → `Qcast-x86_64.AppImage`. Note: the script bakes in
  Fedora-toolchain workarounds and Fedora paths; the job overrides the GStreamer dirs for
  Debian/Ubuntu but the rest is unverified there (see `deploy/CI.md` risk #1).
- **`windows-installer` job (`windows-latest`):** build binary + plugin (MSVC) → gather the
  GStreamer runtime via `gather-payload.ps1` → compile `qcast.iss` with `ISCC.exe` →
  `Qcast-Setup-<ver>.exe`.
- (later) macOS job → `.app` + GStreamer.framework.
- Code signing is **not** wired yet (artifacts ship unsigned; the Windows installer will trip
  SmartScreen) — see `deploy/CI.md` and `deploy/windows/README.md`.

This replaces "build on the user's machine" with "download a prebuilt artifact" — the core win.

---

## 5. Sequencing

1. **Step 1 (relocatable binary)** — embed web client + exe-relative plugin path. **DONE**
   (a copied binary runs outside the repo).
2. **Linux AppImage** — **DONE + validated on Fedora 43.** *Remaining:* test on a second
   distro/container, and the core-runtime-lib bundling follow-up.
3. **Windows installer** — recipe authored (`deploy/windows/`); **still needs** a Windows
   machine/CI to build the payload and test on a clean Windows VM.
4. **CI pipeline** to automate 2 + 3 — **drafted** (`.github/workflows/release.yml`,
   `deploy/CI.md`); not yet run on GitHub Actions.
5. **Signing** (Windows Authenticode, AppImage GPG; macOS notarization when macOS is added) —
   **not started.**

## Licensing note
We prefer **VP8** (royalty-free), so bundling avoids the H.264/openh264 (Cisco binary) licensing
wrinkle. GStreamer core/base/good are LGPL — dynamic bundling is fine. Keep `-ugly`/`-bad`
inclusions minimal and check any codec we actually ship.
