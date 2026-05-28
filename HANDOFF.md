# Overnight handoff — 2026-05-28

Autonomous session toward **finish pivot + installer** (Windows↔Windows remote support).
Everything below is **committed on `master`, builds green, all tests pass** on Fedora.

## The one hard blocker (read first)

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

1. **Boot Windows** (or build a clean Windows VM — say the word and I'll script the
   `virt-install` + a remote-exec bridge so I can drive it from the CLI next time).
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

## Not yet started (tracked)
- `qcast-sender` → library refactor for the Tauri `share` role.
- Pairing secret at the signalling layer (mandatory now that we inject input; current
  short-code gate is client-side only).
