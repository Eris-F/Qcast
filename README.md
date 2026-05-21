# Qcast

Reliability-first desktop screencast. Your machine captures its screen and serves it to
**any web browser on your network** — phone, tablet, or another desktop — with **no install
on the receiving end**. Open a URL (or scan a QR code) and the screen plays in a `<video>`.

Built on **GStreamer `webrtcsink`** (Rust / `gstreamer-rs`), which encodes once and serves
many viewers with per-consumer congestion control, adaptive bitrate, loss recovery (RTX/FEC),
codec negotiation, and DTLS-SRTP encryption — and runs its own signalling + web servers. We
own the served web client, so we can layer custom UI and extra data on top.

Guiding priorities (they shape every decision):

- **Reliability over performance** — *"even if the fps drops, the connection stays."*
- **Browser-only receiver, zero dependencies** — the host serves the HTML/JS client.
- **Capture native, adapt per-viewer** — baseline **1080p**; each browser negotiates the
  codec/bitrate it can handle.
- **Local-first** now; a hosted "from anywhere" path (public reverse-proxy + TLS) is deferred.

## Quick start

```bash
# 1. One-time guided setup (installs GStreamer + builds webrtcsink + the host)
bash deploy/setup-linux.sh           # Linux  (dnf / apt / pacman / zypper)
# or, on Windows, from an elevated PowerShell:
#   powershell -ExecutionPolicy Bypass -File deploy\setup-windows.ps1

# 2. Run it
./target/release/qcast-sender
```

A small window opens with a **system check** plus the **viewer URL + QR code**. Click
**“Start & run in background”**, approve the screen-share picker, and the window closes —
Qcast keeps streaming as a background process (no taskbar entry). Open the URL on any device
on your network. Stop it with the **Ctrl+Alt+Q** global hotkey, or by killing the process
(`pkill -x qcast-sender`). On Wayland the global hotkey can't be registered (compositor
security), so killing the process is the reliable stop there.

For automation / running under a service manager, skip the GUI:

```bash
qcast-sender --headless              # starts streaming immediately, runs until Ctrl+C
qcast-sender --source test           # use a test pattern instead of the real screen
```

## How it stays reliable

- **Decodable 1080p.** The capture is scaled to fit a **1920×1080** box before encoding.
  Browser WebRTC decoders advertise hard frame-size ceilings (Firefox H.264 ≈ 720p; VP8
  ≈ 3.1 MP), so sending a native ultrawide/4K frame decodes *nowhere* — it connects but shows
  black. **VP8 is preferred** (no profile/level ceiling, so 1080p decodes everywhere) with
  **H.264 as a fallback** for devices that advertise a high enough level.
- **Built-in TURN relay.** A small TURN server runs **in-process** (the `turn` crate) and ICE
  is forced through it. This collapses connectivity to a single relay pair, which is the most
  reliable transport *and* sidesteps a libnice candidate-nomination crash that can abort the
  host when a real device offers a full host/srflx/mDNS/ICE-TCP candidate matrix. **No coturn
  to install or configure** — it works the same on every OS.

## Setup scripts

Distribution is a **guided setup that installs dependencies**, not a manual dev-env checklist:

- **`deploy/setup-linux.sh`** — detects the package manager (dnf/apt/pacman/zypper), installs
  the full GStreamer stack + libnice + pipewire + VA drivers + build tools (git, C/C++
  toolchain, cargo-c), builds the gst-plugins-rs `webrtcsink` plugin from source into the user
  plugin dir when it isn't already present, builds the host, and verifies every required
  element. Idempotent. `--verify` re-runs only the checks; `--no-build` skips the cargo build.
  Uses `sudo` for system packages (prompts normally — no passwordless sudo required).
- **`deploy/setup-windows.ps1`** — the same shape in PowerShell (the only shell guaranteed on a
  fresh Windows box). Auto-installs **Git** and the **MSVC C++ build tools** (winget, with
  installer fallbacks), the **GStreamer MSVC runtime + devel** MSIs, Rust, and cargo-c; builds
  webrtcsink + the host; verifies elements. No TURN dependency. `-Verify` / `-NoBuild` mirror
  the Linux flags.

## Capture per platform

| Platform | Source |
|---|---|
| Linux / Wayland | `pipewiresrc` via the xdg-desktop-portal ScreenCast picker |
| Linux / X11 | `ximagesrc` (fallback when the portal is unavailable) |
| Windows | `d3d11screencapturesrc` (Desktop Duplication) |

The encoder is webrtcsink's choice (hardware where the drivers expose it — VAAPI / QuickSync /
Media Foundation / NVENC — else software VP8/openh264).

## Workspace

- `crates/qcast-core` — shared: platform-aware GStreamer element selection + helpers.
- `crates/qcast-sender` — the host: capture → `videoscale` → `webrtcsink`, the served web
  client (`web-client/`), the built-in TURN relay (`turn.rs`), and the pre-launch GUI.
- `crates/qcast-server` — WebSocket rendezvous for the future hosted "from anywhere" mode
  (deferred; not needed for LAN).

## Platform status

- **Linux** — working and tested (Fedora 43 / GStreamer 1.26). Intel-VAAPI laptop is the
  cross-platform target.
- **Windows** — code + installer written; **not yet validated on a Windows machine** (the
  first run there is the validation).
- **macOS** — not currently targeted.

## Build (without the setup script)

You need GStreamer **1.26** with dev headers, libnice, and the gst-plugins-rs `webrtcsink`
plugin available on the plugin path. The setup scripts handle all of that; if you're building
by hand:

```bash
cargo build --release -p qcast-sender
```
