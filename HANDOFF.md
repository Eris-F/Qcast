# Overnight handoff — 2026-05-28

Autonomous session toward **finish pivot + installer** (Windows↔Windows remote support).
Everything below is **committed on `master`, builds green, all tests pass** on Fedora.

## 🎉 The installer is BUILT

`~/qcast-vm/Qcast_0.1.0_x64-setup.exe` — **273 MB NSIS `-setup.exe`**, produced by
`cargo tauri build` inside a rootless-podman/KVM Windows 11 VM stood up tonight.
File magic confirms a real Nullsoft installer.
SHA256: `f382b2af82adad6f51d395ec430034fb71f98ef44c92bb2ef03e06ce00bceb6e`.
**Bundle contents (verified via `7z l`):** `qcast-app.exe` (9.4 MB Tauri shell) +
`lib/gstreamer-1.0/gstrswebrtc.dll` (18.8 MB, gst-plugins-rs webrtcsink) + the
`libexec/gstreamer-1.0/gst-plugin-scanner.exe` + 100+ GStreamer runtime/plugin DLLs +
the WebView2 offline runtime.

How it got built, end-to-end and autonomous (no sudo, no Windows reboot):

1. `flatpak install --user org.gnome.Boxes` → scriptable `qemu-system-x86_64` v7.2 +
   OVMF UEFI inside the flatpak; `/dev/kvm` was world-accessible (0666).
2. `dockur/windows` (rootless podman + `/dev/kvm` + `USER_PORTS=22`) downloaded the
   official Win11 ISO + ran an unattended install with `/oem/install.bat` enabling
   OpenSSH.
3. SSH'd in (`qcast:qcastpass@localhost:2222`), `scp`'d the repo, installed the
   toolchain (VS C++ Build Tools, Rust MSVC, GStreamer 1.26 MSVC runtime+devel via
   official MSIs, cargo-c, `cargo install tauri-cli`).
4. **First-ever Windows compile of `inject_windows.rs` caught exactly one type
   mismatch** (`mouseData: i32` → `u32`); fix committed (`ded4de8`), then
   `cargo build -p qcast-sender` succeeded + **all 32 unit tests passed on Windows.**
5. Authored the Tauri scaffold (`src-tauri/`, independent workspace so Fedora root
   build stays green; default icons from `tauri-apps/tauri` MIT template), staged 391
   GStreamer DLLs + scanner into `src-tauri/gst-runtime/`, ran `cargo tauri build` →
   the bundle above.

6. **Caveat-fix:** cloned gst-plugins-rs @ 0.15 in the guest, ran `cargo cbuild -p
   gst-plugin-webrtc`, dropped the resulting `gstrswebrtc.dll` (18.8 MB) into
   `gst-runtime/lib/gstreamer-1.0/`, re-ran `cargo tauri build` → the bundle above
   (v2). The installer is now functionally complete; `webrtcsink` is present.

**Remaining for true sign-off:** running `Qcast_0.1.0_x64-setup.exe` on a **clean**
Windows machine (one that does NOT have the system GStreamer install that the build VM
has) and confirming (a) it installs without UAC + (b) Qcast launches + (c) `share` +
`connect` work end-to-end. The build VM is not a valid validation target (its system
GStreamer would mask the bundled one). Procedure: `deploy/WINDOWS_INSTALLER.md` §9 +
`deploy/TEST_PLAN.md` Layer 5.

## The "Windows hardware" blocker that was here

There is **no Windows test environment on this box**: Windows 25H2 is a bare-metal
**dual-boot**, not a VM, and no virt tooling is installed. A dual-boot can't be driven
from the running Fedora session, so **all Windows-specific validation is blocked until
you boot Windows** (or we build a clean Windows VM — needs install media; none in
`~/Downloads`). Also: Tauri itself can't build on Fedora (`webkit2gtk-4.1`/`gtk3`/
`cargo-tauri` missing, no passwordless sudo).

So tonight I did **everything that's provable/authorable on Fedora** and left the
Windows parts as ready-to-run + a tight validation checklist.

## What's done (commits, newest first)

| Commit | What |
| --- | --- |
| `docs(tauri)` | Tauri app scaffold blueprint — `deploy/tauri/` (conf + README) incl. tray + branding |
| `feat(receiver)` | Receiver forwards mouse/keyboard over the data channel (`attachVideoElement`) |
| `test(input)` | `deploy/TEST_PLAN.md` + `QCAST_INPUT_LOG` file injector for future E2E |
| `feat(input)` | **Decode webrtcsink navigation events → injectable input** (the core remote-control loop) |
| `feat(windows)` | Installer derisk: H.264 fallback plugins + `deploy/WINDOWS_INSTALLER.md` decision doc |
| `feat(bundle)` | `bundle.rs`: Tauri `resources\` layout + auto bundle-mode on Windows |

### Pivot core — remote control (the novel hard part) ✅ implemented + tested on Fedora
- **Sender half** (`crates/qcast-sender/src/input/`): webrtcsink `enable-data-channel-navigation`
  → upstream `GstNavigation` events → decoded to a typed `InputEvent` → injected.
  Windows `SendInput` backend (`KEYEVENTF_UNICODE` typing + control-key map + absolute
  mouse) behind `#[cfg(windows)]`; logging no-op elsewhere.
- **Receiver half** (`web-client/app.js`): `session.attachVideoElement(video)` forwards
  mouse/keyboard/scroll. Video got `tabindex=0` for keyboard focus.
- **Proven on Fedora (deterministic, no browser/webrtcsink):** an integration test sends
  an upstream nav event through a real pad probe and asserts it's decoded + injected
  (`upstream_navigation_event_reaches_the_injector`), plus parser/dispatch unit tests.
  10 input tests, ran 3× stable.

### Installer ✅ derisked + scripted (build/validate on Windows)
- Decision doc `deploy/WINDOWS_INSTALLER.md`: **NSIS + `installMode: currentUser`
  (per-user, no UAC) + WebView2 `offlineInstaller`** (Prototype A, primary) vs
  Inno-wraps-Tauri-exe (Prototype B, fallback). Two research threads' findings, the
  flat-DLL-search risk + mitigations, signing/SmartScreen, cargo-xwin cross-build note.
- `bundle.rs`: handles both the flat (Inno) and `resources\` (Tauri) layouts; on Windows
  a found bundled dir auto-clears the system plugin path (closes a real gap — a shortcut
  can't set `QCAST_BUNDLE=1`). New unit test.
- `gather-payload.ps1`: added the **missing LGPL H.264 fallback** (`gstmediafoundation`,
  `gstopenh264`, `gstvideoparsersbad`); VP8 stays preferred; never GPL `x264`.

### Tauri shell 📐 blueprinted (can't build on Fedora)
- `deploy/tauri/` — `tauri.conf.json` template + a copy-pasteable build-on-Windows guide:
  Path A, one binary role-at-launch (`share`/`connect`), GStreamer-as-resources, and
  **your requested tray-not-taskbar + configurable name/icon/title** (Rust snippets,
  within the consent model — identifiable + one-click "Stop sharing", not concealment).

## Do this when you're back (in order)

1. **Boot Windows** and run `deploy/tauri/build-windows.ps1` (fastest one-off) — **or**
   stand up the VM so I can finish it autonomously: `deploy/vm/` now has
   `create-windows-vm.sh` + `windows-setup.ps1`. You provide a Windows ISO + run
   `sudo dnf install @virtualization` (the only steps I can't), then hand me the
   guest's SSH IP and I'll build + validate the installer over SSH.
2. Provision the build box per `deploy/windows/README.md` §Prereqs + `cargo install tauri-cli`.
3. `gst-inspect-1.0` smoke-check `d3d11screencapturesrc`, `webrtcsink`, `rtpgccbwe`,
   `vp8enc`, an H.264 element (TEST_PLAN.md Layer 3).
4. `cargo build -p qcast-sender` on MSVC — **first real compile of `inject_windows.rs`**
   (we couldn't on Fedora). Fix any windows-crate API drift.
5. Create the Tauri app per `deploy/tauri/README.md`; `cargo tauri build` → the NSIS
   installer. Validate per `WINDOWS_INSTALLER.md` §9.
6. End-to-end: `share` + `connect`, confirm video + remote mouse/keyboard. Use
   `QCAST_INPUT_LOG` + Playwright to make it a repeatable test (TEST_PLAN.md Layer 4).

## Top risks to watch (all Windows-side)
1. Flat GStreamer loader DLLs vs Tauri's `resources\` subfolder (DLL search path).
2. `inject_windows.rs` compiles (unverified — no MSVC cross-target on Fedora).
3. H.264 fallback actually negotiates; GIO TLS modules for DTLS on a clean box.
4. The browser→webrtcsink data-channel→upstream-event link (everything after it is proven).

## Done since first draft
- **`qcast-sender` → library split** for the Tauri `share` role: `qcast_sender` lib now
  exposes the host/TURN/bundle/input logic; `main.rs` is a thin wrapper. The Tauri app
  can `use qcast_sender::{host, bundle, input}` directly. Bin unchanged, all green.
- **Scroll-wheel** remote input (`InputEvent::MouseScroll` + Windows `WHEEL`/`HWHEEL`).

## Not yet started (tracked)
- Pairing secret at the signalling layer (mandatory now that we inject input; the
  current short-code gate is client-side only). The one remaining non-Windows item —
  deferred as a security-sensitive webrtcsink-signaller change best done with validation.
