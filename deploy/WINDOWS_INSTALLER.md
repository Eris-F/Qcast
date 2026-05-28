# Qcast — Windows bundled installer (drag-and-install)

> **Goal:** a Windows user double-clicks one downloaded installer, clicks through,
> and runs Qcast — **no separate GStreamer install, no WebView2 install, no Rust /
> MSVC / compile step, no admin prompt.** Windows↔Windows only for now (Linux
> deferred); the receiver consumes the WebRTC stream **inside the Tauri WebView2**
> (Path A), the sender runs the GStreamer capture pipeline.
>
> **Status (2026-05-28):** DESIGNED + DERISKED on the Fedora dev box; **not yet
> built or run on Windows** (this box is a Linux/Windows dual-boot, so the installer
> can't be produced or validated here). Every claim below marked ⚠️ needs a Windows
> validation pass — see the checklist at the end. This doc supersedes the
> Windows section of `PACKAGING.md` for the Tauri/Path-A pivot.

---

## 1. The artifact has changed

The existing `deploy/windows/` recipe (Inno Setup) was written for the **old egui
`qcast-sender.exe`** GUI. The pivot changes two things:

1. The app is now a **Tauri v2 app** (Rust backend + a WebView2 frontend that
   consumes the stream). → **WebView2 runtime must be handled** (the old recipe
   didn't, because egui needs no webview).
2. **One binary, role at launch** (`share` vs `connect`). The installer ships one
   exe; both roles live in it.

Everything else the old recipe got right is **reused**: relocatable GStreamer
bundling via `crates/qcast-sender/src/bundle.rs`, the curated Windows plugin set,
per-user/no-UAC install, the signing/SmartScreen notes.

---

## 2. Two prototypes (build both if the first hits a wall)

### Prototype A — Tauri NSIS bundler  *(RECOMMENDED primary)*

Let Tauri's own bundler produce an NSIS `-setup.exe`, and inject GStreamer as
bundled resources.

```jsonc
// src-tauri/tauri.conf.json
"bundle": {
  "active": true,
  "targets": ["nsis"],
  "resources": {
    // array/dir form preserves the tree; map+glob FLATTENS — see risk #1.
    "gst-runtime/lib/gstreamer-1.0": "lib/gstreamer-1.0",
    "gst-runtime/libexec/gstreamer-1.0/gst-plugin-scanner.exe": "libexec/gstreamer-1.0/gst-plugin-scanner.exe"
    // the flat top-level runtime DLLs: see risk #1 for where these must land
  },
  "windows": {
    "nsis": { "installMode": "currentUser" },           // per-user, NO admin, %LOCALAPPDATA%
    "webviewInstallMode": { "type": "offlineInstaller", "silent": true }
  }
}
```

- **Pros:** one integrated installer; WebView2 handled automatically; per-user no-UAC
  (NSIS `currentUser` — WiX/MSI *cannot* do per-user, it always triggers UAC);
  **cross-buildable from Linux** via `cargo-xwin` (so the Fedora box can produce it).
- **Cons / the catch:** Tauri puts `bundle.resources` under
  `<install>\resources\…`, **not flat beside the exe**. Our bundled GStreamer
  plugins go to `<install>\resources\lib\gstreamer-1.0\` — `bundle.rs` now has a
  candidate path for exactly this (see §4). BUT the **flat loader DLLs**
  (`gstreamer-1.0-0.dll`, `glib-2.0-0.dll`, …) must be on the **exe's DLL search
  path**, and `resources\` is a subfolder Windows won't search by default → **risk #1**.

### Prototype B — Inno Setup wraps the Tauri-built exe  *(proven-layout fallback)*

Keep the existing `deploy/windows/qcast.iss` + `gather-payload.ps1` flow, but stage
the **Tauri-built exe** instead of the egui one, and add WebView2.

- **Pros:** uses the **already-correct flat layout** (`{app}\*.dll`,
  `{app}\lib\gstreamer-1.0`, `{app}\libexec\…`) that `bundle.rs` resolves today with
  zero subfolder/DLL-search surprises — **sidesteps risk #1 entirely.**
- **Cons:** WebView2 is manual — add the **Evergreen Bootstrapper**
  (`MicrosoftEdgeWebview2Setup.exe`, ~2 MB online, or the offline installer) to
  `[Files]` + a `[Run]` entry, or check the `pv-RegKey` and download on first run.
  Inno `ISCC.exe` only compiles on Windows (or under Wine — compile-only).

**Recommendation:** pursue **A** first (cleaner, auto-WebView2, Linux-cross-buildable).
If risk #1 (flat DLL search) proves painful, fall back to **B**, whose flat layout
is already proven against `bundle.rs`.

---

## 3. GStreamer bundling — settled facts (from packaging research)

- **Source:** official **MSVC x86_64 MSIs** from gstreamer.freedesktop.org —
  runtime *and* devel (devel only on the build box for cargo-c / linking; not
  shipped). Install `ADDLOCAL=ALL` (the default "Typical" omits plugins).
- **Relocatable, registry-free, private deployment** (copy DLLs with the app) — NOT
  the merge-module (`.msm`) route (registry-touching, heavier).
- **Licensing: LGPL-clean.** VP8 (`gstvpx.dll`) is the preferred codec. H.264
  fallback uses **`gstmediafoundation.dll`** (native Windows HW, LGPL) +
  **`gstopenh264.dll`** (Cisco SW, LGPL) + **`gstvideoparsersbad.dll`** (`h264parse`).
  **NEVER `gstx264.dll` (GPL)** or any `*-gpl` package. (`gather-payload.ps1` now
  includes the three H.264 DLLs — they were missing.)
- **Ship the whole `bin\*.dll`** (~80–120 MB). The transitive closure (glib, gobject,
  gio, gmodule, intl, ffi, zlib, orc, gnutls/nettle/openssl chains, usrsctp, nice,
  libsrtp2, the gst core/base/video/audio/rtp/webrtc/sdp/app/pbutils/net libs) is
  wide and brittle; trim later only with a **recursive `Dependencies.exe` trace of
  the exe AND every staged plugin DLL**, re-validating the live pipeline after each cut.
- **Do NOT ship a prebuilt `registry-*.bin`** — it keys on absolute path+mtime+size
  and won't match on the target; it regenerates to a per-user writable cache
  (`%LOCALAPPDATA%\…\gstreamer-1.0\registry-x86_64.bin`) on first run. Fine under a
  per-user install.
- `gstvideoconvertscale.dll` is the single correct DLL for both `videoconvert` and
  `videoscale` in 1.26 (standalone DLLs no longer exist). `gstd3d11.dll` covers both
  `d3d11screencapturesrc` and `d3d11download`. `gstrsrtp.dll` is `rtpgccbwe`'s home.

---

## 4. `bundle.rs` changes already made (Fedora-green)

`configure_plugin_path()` (runs before `gst::init()`) now:

1. **Adds the Tauri `resources\` candidate path** — `<exedir>\resources\lib\gstreamer-1.0`
   (+ scanner under `<exedir>\resources\libexec\…`), alongside the existing AppImage
   (`../lib`), Inno-flat (`lib`), and bare-flat candidates. Covered by a unit test.
2. **On Windows, a found bundled dir implies bundle-mode** — clears
   `GST_PLUGIN_SYSTEM_PATH_1_0` **without needing `QCAST_BUNDLE=1`**. This closes the
   highest-impact gap research flagged: a Start-Menu/installer shortcut can't set env
   vars, so the old `QCAST_BUNDLE=1`-gated clearing never fired on Windows, letting a
   host's mismatched GStreamer leak in. (Linux/AppImage keeps the explicit opt-in.)

Still **not** handled in `bundle.rs` (see risks): the flat loader-DLL search path
(risk #1) and GIO TLS modules (risk #3).

---

## 5. WebView2

Use **`webviewInstallMode: offlineInstaller`** (~127 MB added). It guarantees install
with **no internet on the target** (friend-group constraint) while keeping an
**evergreen** WebView2 that Microsoft patches — `fixedVersion` is ~50 MB larger and
makes *us* responsible for security patches; `downloadBootstrapper` (the Tauri
default) needs internet. For Prototype B, ship the Evergreen Bootstrapper in Inno.

---

## 6. Code signing & SmartScreen

Unsigned installers downloaded via a browser get Mark-of-the-Web → SmartScreen
**"Windows protected your PC / Unknown publisher"**; the user clicks **More info →
Run anyway**. For a trusted friend group this is acceptable — distribute the
`-setup.exe` directly (not zipped) with those two click instructions. Cheapest real
fix later: **Azure Trusted Signing (~$10/mo)** via Tauri's `bundle.windows.signCommand`
(works from Linux; the built-in signer is Windows-only). OV certs still warn until
reputation builds; only EV gets immediate trust.

---

## 7. Cross-building from Linux (optional, to produce A on this box)

NSIS (not MSI) **can** cross-build from Linux: install `nsis`, `clang`, `lld`,
`llvm`, the `x86_64-pc-windows-msvc` Rust target, and **`cargo-xwin`** (downloads
the Windows SDK/CRT). ⚠️ **Biggest cross-build unknown:** the GStreamer Rust
bindings must link **MSVC-built** GStreamer — `cargo-xwin` would need the MSVC
GStreamer dev libs reachable by the linker (`pkg-config`/lib paths), which we don't
have on Fedora. Also: Fedora's `nsis` package may ship the binary without
stubs/plugins. **Treat cross-build as a stretch; the reliable path is building A or B
on Windows.**

---

## 8. Risks to validate on Windows (ordered by impact)

1. **⚠️ Flat loader-DLL search path (Prototype A).** The ~dozens of top-level
   GStreamer runtime DLLs must be findable by the exe's loader. Options, in order of
   preference: **(a)** an NSIS `bundle.windows.nsis.installerHooks` `.nsh`
   (`NSIS_HOOK_POSTINSTALL`) that copies the flat runtime DLLs from `resources\` up
   to `$INSTDIR` next to the exe; **(b)** call `SetDllDirectoryW`/`AddDllDirectory`
   at startup pointing at the resources dir (Windows-only Rust, a few lines); **(c)**
   use Prototype B's flat layout. Decide by testing (a) first.
2. **⚠️ H.264 fallback** actually negotiates with the bundled `gstmediafoundation.dll`
   (+ `gstvideoparsersbad.dll`). Confirm which H.264 element `webrtcsink`'s codec
   preference instantiates; VP8 is the default so this only bites a peer that forces H.264.
3. **⚠️ GIO TLS modules.** DTLS via `gstdtls.dll` uses OpenSSL directly (bundled in
   `bin\`), so it *may* be fine — but if DTLS handshakes fail on a clean box, ship
   `lib\gio\modules\*.dll` + set `GIO_EXTRA_MODULES`. Not handled today.
4. **⚠️ `gst-plugin-scanner` writes its cache** under the per-user install (it should;
   verify it doesn't try `{app}`).
5. **⚠️ Resource map glob flattening** — confirm plugins land in nested
   `resources\lib\gstreamer-1.0\`, not flattened (use the array/dir form, not map+glob).
6. **⚠️ `cargo-xwin` MSVC linkage** against bundled GStreamer (only if cross-building).

---

## 9. Windows validation checklist (run after a `git pull` on the Windows boot)

On the build box (VS 2022 C++ tools, GStreamer 1.26 MSVC runtime+devel ADDLOCAL=ALL,
Rust MSVC, cargo-c, gst-plugins-rs checkout, + Inno Setup *or* the Tauri NSIS toolchain):

1. **`gst-inspect-1.0`** each critical element against the install:
   `d3d11screencapturesrc`, `webrtcsink`, `rtpgccbwe`, `vp8enc`, and the chosen H.264
   element — all green.
2. **Build the installer** (A: `tauri build`; B: `gather-payload.ps1` → `ISCC.exe qcast.iss`).
3. On a **clean Windows 11** (no GStreamer, no Rust, ideally a fresh VM):
   - Run the `-setup.exe`. Confirm **no UAC prompt** (per-user) and the SmartScreen
     "More info → Run anyway" flow is the only friction.
   - Launch Qcast. Pick **share**; start `--source test`; from a second machine run
     **connect** and confirm the test stream plays in the WebView2 — proves the
     bundled plugins (incl. `gstrswebrtc.dll`, vpx, WebRTC transport) loaded from the
     bundle with **no system GStreamer present**.
   - Move the mouse / type in the receiver and confirm input reaches the sender
     (the `GstNavigation` → `SendInput` path).
   - Real desktop capture (no `--source test`): confirm `d3d11screencapturesrc` works.
4. If a host has a **different** GStreamer version and plugins conflict, confirm the
   new `bundle.rs` Windows bundle-mode (system path cleared automatically) prevents it.

---

## Sources
- Tauri v2 Windows installer / resources / signing / config reference — https://v2.tauri.app/distribute/windows-installer/, /develop/resources/, /distribute/sign/windows/, /reference/config/
- WiX per-user limitation — https://github.com/tauri-apps/tauri/issues/13792 · WebView2 modes — https://github.com/orgs/tauri-apps/discussions/7035
- GStreamer Windows install/deploy/registry/d3d11/videoconvertscale/rtpgccbwe docs — https://gstreamer.freedesktop.org/documentation/
