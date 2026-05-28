# Qcast — test plan & regression net

Goal: catch regressions reliably and keep the Linux dev box **green** even though the
product target is **Windows↔Windows**. Principles (from the project working mode):
**deterministic, non-interactive** (never the Wayland portal picker — always
`--source test`/videotestsrc), and **leave every change green**.

The hard constraint: this dev box is a Linux/Windows **dual-boot**, so Windows-only
behavior (SendInput, d3d11 capture, WebView2, the installer) **cannot be tested from
the running Linux session** — those layers need a Windows boot or a clean Windows VM.
This plan separates "tested now on Fedora" from "needs Windows" and proposes
automation to shrink the manual surface.

---

## Layer 1 — Unit (runs now, Fedora, deterministic)

| Area | Test | File |
| --- | --- | --- |
| Navigation decode | mouse-move normalization, button press/release, key press/release, non-nav rejection, coord clamping | `crates/qcast-sender/src/input/event.rs` |
| Dispatch | decode→inject happy path; non-navigation is ignored | `crates/qcast-sender/src/input/mod.rs` |
| Relocatable bundle | AppImage/flat/Tauri-resources layout detection; no-op when unbundled | `crates/qcast-sender/src/bundle.rs` |
| Codec/resolution | codec-pref → caps mapping + caps parse; resolution validation | `crates/qcast-sender/src/host.rs` |
| Supervisor | restart/backoff/give-up trajectories | `crates/qcast-sender/src/host.rs` |
| Access code | shape + unambiguous alphabet | `crates/qcast-sender/src/access_code.rs` |

Run: `cargo test -p qcast-sender` (also `cargo test --workspace`). **Must stay green.**

## Layer 2 — Integration on Fedora (runs now)

- **Navigation probe wiring (deterministic):** `upstream_navigation_event_reaches_the_injector`
  builds `videotestsrc ! fakesink`, attaches the real probe, sends an upstream
  `GstNavigation` event, and asserts it is decoded + injected. This is the regression
  net for the probe ↔ dispatch ↔ injector path — **no `webrtcsink` or browser needed.**
- **Host integration** (`tests_integration.rs`, `#[ignore]`, skip when `webrtcsink`
  absent): real pipeline build + TURN bind + HTTP serve + `session.json`. Run with
  `cargo test -p qcast-sender -- --ignored`.

## Layer 3 — Needs Windows: input injection (`SendInput`)

`crates/qcast-sender/src/input/inject_windows.rs` compiles only under `#[cfg(windows)]`
and is **unvalidated**. On the Windows boot:

1. `cargo build -p qcast-sender` (MSVC) — confirm it compiles (catches windows-crate
   API drift; we couldn't compile-check it on Fedora — no MSVC cross-target).
2. Manual: run the sender, connect a receiver, and verify mouse move/click + typing
   (incl. a non-US character → proves `KEYEVENTF_UNICODE` layout-proofing) and the
   control keys (Enter/Backspace/Tab/arrows) land on the sender.
3. **Proposed automation (makes this repeatable):** add a `QCAST_INPUT_LOG=<path>`
   env that swaps in a *file-logging* injector which appends each decoded
   `InputEvent` as JSON. A Playwright test (Layer 4) drives the receiver, then asserts
   the file's contents — turning the browser→sender path into an automatable check
   without real desktop side effects. Small to add; do this before the manual pass.

## Layer 4 — Needs a browser: the data-channel navigation transport

The one link not yet exercised: browser/WebView2 → `webrtcsink` navigation data
channel → upstream `GstNavigation` event. It's a documented `webrtcsink` feature, and
our probe (Layer 2) already proves everything from the upstream event onward.

To validate / automate:
1. **Receiver JS:** enable `gstwebrtc-api` navigation forwarding in the receiver
   (bind the consumer session to the `<video>` element so mouse/keyboard are sent over
   the data channel). Currently `web-client/app.js` is view-only — this is part of the
   Tauri receiver scaffold.
2. **Headless E2E (proposed harness):** `cargo run -p qcast-sender -- --headless
   --source test` with `QCAST_INPUT_LOG` set → Playwright (Chromium, which matches
   WebView2's engine) loads the receiver, connects, dispatches mouse/key events on the
   video element → assert the log file contains the expected `InputEvent`s. This runs
   on **Fedora** (Chromium ≈ WebView2 for WebRTC), closing the gap without Windows.
   Needs: `npm i -D playwright` + `npx playwright install chromium` (network), and the
   receiver JS from step 1. Not done tonight (network + JS pending the Tauri scaffold).

## Layer 5 — Needs Windows: the bundled installer

See `deploy/WINDOWS_INSTALLER.md` §9 for the full checklist. Key asserts: per-user
install with no UAC; launches with **no system GStreamer**; `gst-inspect-1.0` greens
`d3d11screencapturesrc`/`webrtcsink`/`rtpgccbwe`/`vp8enc`/H.264; test-pattern stream
plays in the receiver; remote input works; real d3d11 capture works.

---

## The Windows↔Windows test ladder

- **T0 — one Windows box, loopback.** Both roles (`share` + `connect`) on one machine
  over localhost, `--source test`. Exercises consume + data channel + nav → SendInput.
- **T1 — two Windows boxes on a LAN.** Real two-machine remote control.
- **T2 — clean Windows VM.** Installer validation on a pristine machine (no GStreamer).
- **T3 — a friend's real machine.** Real-world internet/NAT + capture hardware.

Safe injection testing: control a **VM** (sender in the VM, receiver on the host) so
injected input stays sandboxed and can't clobber your own session.

## CI (future)

- Keep the existing Linux CI green (build + `cargo test --workspace`).
- Add a **Windows CI job** (`windows-latest`): `cargo build`/`cargo test` so the
  `cfg(windows)` SendInput code is at least compiled on every push (the gap we can't
  cover on Fedora). Optionally `tauri build` to smoke the installer.
