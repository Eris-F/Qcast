# Qcast

Reliability-first desktop screencast. Your machine captures its screen and serves it to
**any web browser on your network** — phone, tablet, or another desktop — with **no install
on the receiving end**. Open a URL (or scan a QR code), enter the password the host shows,
and the screen plays in a `<video>`.

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

### Linux — prebuilt AppImage (recommended)

Download (or build, below) `Qcast-x86_64.AppImage`, then:

```bash
chmod +x Qcast-x86_64.AppImage
./Qcast-x86_64.AppImage
```

One self-contained file — no GStreamer install, no toolchain, no compile. It bundles the
host plus the GStreamer plugins it needs (including our `webrtcsink`/`webrtcbin` and the
congestion-control element). See **Platform status** for what's validated.

### Windows — installer

A per-user installer (`Qcast-Setup-<version>.exe`, Inno Setup) that bundles the binary,
the GStreamer runtime DLLs, the plugins, and our plugin — Start-Menu shortcut, no admin
prompt, no toolchain. **Note:** the installer recipe is written but has **not yet been
built or tested on Windows** (see Platform status); until then, build from source.

### Build from source (contributors / unsupported platforms)

A one-time guided setup installs GStreamer + builds the `webrtcsink` plugin + the host:

```bash
bash deploy/setup-linux.sh           # Linux  (dnf / apt / pacman / zypper)
# or, on Windows, from an elevated PowerShell:
#   powershell -ExecutionPolicy Bypass -File deploy\setup-windows.ps1

./target/release/qcast-sender
```

## Running it

A small window opens with a **system check**, the **viewer URL + QR code**, the **viewer
password**, and a few pre-launch options. Click **"Start & run in background"**, approve the
screen-share picker, and the window closes — Qcast keeps streaming as a background process
(no taskbar entry). Stop it with the **Ctrl+Alt+Q** global hotkey, or by killing the process
(`pkill -x qcast-sender`). On Wayland the global hotkey can't be registered (compositor
security), so killing the process is the reliable stop there.

For automation / running under a service manager, skip the GUI:

```bash
qcast-sender --headless              # starts streaming immediately, runs until Ctrl+C
qcast-sender --source test           # use a test pattern instead of the real screen
```

In `--headless` mode the generated viewer password is printed to the log.

## How viewing works

1. On the host, note the **URL** (or show the QR code) and the **password** — they're shown
   separately on purpose; the password is **not** embedded in the URL or QR.
2. On any device on the network, open the URL (or scan the QR with a phone camera).
3. The browser shows a **password screen**. Enter the code (it's case- and
   separator-insensitive — `ghf aba 6tj`, `GHF/ABA/6TJ`, and `ghfaba6tj` all match).
4. The stream starts. The viewer has a dark UI with a live connection-status indicator and
   a fullscreen toggle.

**About the password:** the host generates a fresh random code (format `GHF/ABA/6TJ`) each
run. The browser starts **no** WebRTC connection until a matching code is entered. This is a
**client-side UX gate, not signalling-layer-enforced authentication** — the expected code is
served to the page (in `session.json`), so a determined user on the LAN can read it or talk
to the signalling server directly to bypass the gate. It keeps casual viewers out; **enforced
auth (rejecting unauthenticated consumer sessions at the signalling layer) is future work.**

## Pre-launch options

Set in the GUI before **Start**, or via CLI flags. (Options are applied at start; live
runtime control while streaming is deferred.)

| Option | GUI | CLI |
|---|---|---|
| Resolution cap | `720p` / `1080p` presets, or **Advanced (custom)** width×height | `--max-width` / `--max-height` |
| Codec preference | `VP8 preferred` / `H.264 preferred` / `VP8 only` / `H.264 only` | `--codec auto\|h264\|vp8-only\|h264-only` |
| Viewer password | shown, with a **Regenerate** button | (auto-generated each run) |

Other flags: `--host` (bind address, default `0.0.0.0`), `--web-port` (default `8080`),
`--signalling-port` (default `8443`), `--source auto\|test`, `--headless`.

The resolution cap defaults to a **1080p** box, the universally-decodable baseline. Choosing
a **custom resolution above 1080p** surfaces a warning: browser WebRTC decoders have hard
frame-size ceilings (Firefox H.264 ≈ 720p; VP8 ≈ 3.1 MP), so a larger frame may decode on
some browsers and not others. `--codec auto` (the default) offers both codecs with **VP8
preferred**, since VP8 has no profile/level ceiling and decodes a 1080p frame everywhere.

## How it stays reliable

- **Decodable 1080p.** The capture is scaled to fit a **1920×1080** box (configurable) before
  encoding. Sending a native ultrawide/4K frame decodes *nowhere* on browsers that advertise
  a lower ceiling — it connects but shows black. **VP8 is preferred** (no profile/level
  ceiling, so 1080p decodes everywhere) with **H.264 as a fallback** for devices that
  advertise a high enough level.
- **Built-in TURN relay.** A small TURN server runs **in-process** (the `turn` crate) and ICE
  is forced through it. This collapses connectivity to a single relay pair, which is the most
  reliable transport *and* sidesteps a libnice candidate-nomination crash that can abort the
  host when a real device offers a full host/srflx/mDNS/ICE-TCP candidate matrix. **No coturn
  to install or configure** — it works the same on every OS. (If something is already on the
  TURN port, Qcast reuses it instead of binding a second one.)
- **Self-supervising pipeline.** The running host watches the pipeline and **auto-restarts it
  on a transient error or EOS**, with bounded exponential backoff. It **reuses the captured
  source** across restarts, so a recovery never re-pops the screen-share picker. A run that
  stays healthy resets the restart budget; if the pipeline can't be recovered within the
  budget, the host **stops cleanly** rather than lingering as if it were still streaming.

## Capture per platform

| Platform | Source |
|---|---|
| Linux / Wayland | `pipewiresrc` via the xdg-desktop-portal ScreenCast picker |
| Linux / X11 | `ximagesrc` (fallback when the portal is unavailable) |
| Windows | `d3d11screencapturesrc` (Desktop Duplication) |

The encoder is webrtcsink's choice (hardware where the drivers expose it — VAAPI / QuickSync /
Media Foundation / NVENC — else software VP8/openh264).

## Platform status

- **Linux** — working and validated (Fedora 43 / GStreamer 1.26), including real WebRTC
  playback to devices and the prebuilt AppImage. Intel-VAAPI laptop is the cross-platform
  target. AppImage cross-distro portability is not yet verified (see `deploy/PACKAGING.md`).
- **Windows** — capture code + the bundled installer recipe are written; **not yet built or
  validated on a Windows machine** (the first run there is the validation). Build from source
  for now.
- **macOS** — not currently targeted.

## Packaging

The AppImage (Linux) and the installer (Windows) are the **end-user** path; the
`deploy/setup-*.sh` / `setup-*.ps1` scripts are the **contributor / build-from-source** path.
A draft GitHub Actions pipeline builds both artifacts on a release tag.

- **Linux AppImage** — `deploy/appimage/build-appimage.sh` → `Qcast-x86_64.AppImage`.
- **Windows installer** — Inno Setup recipe in `deploy/windows/` (`gather-payload.ps1` +
  `qcast.iss` + its `README.md`).
- **CI** — draft release workflow at `.github/workflows/release.yml` (details in
  `deploy/CI.md`); not yet validated on a real run.

See **`deploy/PACKAGING.md`** for the full packaging spec and current status of each piece.

## Setup scripts (build from source)

Distribution to contributors is a **guided setup that installs dependencies**, not a manual
dev-env checklist:

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

## Workspace

- `crates/qcast-core` — shared: platform-aware GStreamer element selection + helpers.
- `crates/qcast-sender` — the host: capture → `videoscale` → `webrtcsink`, the served web
  client (`web-client/`, **embedded into the binary** so the artifact is self-contained), the
  built-in TURN relay (`turn.rs`), the pipeline supervisor, and the pre-launch GUI.
- `crates/qcast-server` — WebSocket rendezvous for the future hosted "from anywhere" mode
  (deferred; not needed for LAN).

The binary is **relocatable**: the web client is embedded (`include_dir`) and extracted to a
runtime dir at startup, and the binary resolves bundled GStreamer plugins relative to its own
location — so it runs outside the build tree (which is what makes the AppImage / Windows
bundle work). Set `QCAST_WEB_CLIENT_DIR` to a directory to serve the web client live from
there during web-client development (no recompile).

## Build & test (contributors)

The setup scripts handle the toolchain; to build/test by hand you need GStreamer **1.26** with
dev headers, libnice, and the gst-plugins-rs `webrtcsink` plugin on the plugin path.

```bash
cargo build --release -p qcast-sender   # build the host
cargo test --workspace                   # unit tests (no plugin / ports needed)
cargo test -p qcast-sender -- --ignored  # heavier integration tests (bind ports, need webrtcsink)
```

The `#[ignore]`d integration tests start a real test-pattern host, hit the served web client +
`session.json` over HTTP, exercise the resolution/codec options, and check the TURN relay
lifecycle; they **skip gracefully** when the `webrtcsink` plugin isn't present.
