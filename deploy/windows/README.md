# Qcast Windows bundled installer

Builds `Qcast-Setup-<version>.exe`: a **per-user** installer that ships the prebuilt
`qcast-sender.exe` + the GStreamer MSVC runtime DLLs + the curated plugin set
(including our `gstrswebrtc.dll`) + the `gst-plugin-scanner`. The end user installs
and runs Qcast from the Start Menu with **no winget / Rust / MSVC / compile step**.

This directory is **authoring only**. The payload must be assembled on a Windows
machine (the GStreamer MSVC runtime and the MSVC-built binary/plugin only exist
there). The scripts here have not yet been run on Windows — treat the first run as
the validation. See `deploy/PACKAGING.md` §3 for the spec this implements.

## Files

| File | Purpose |
| --- | --- |
| `gather-payload.ps1` | Builds/locates the exe + our plugin, copies the GStreamer runtime + curated plugins + scanner into a `staging\` dir laid out exactly as `{app}`. |
| `qcast.iss` | Inno Setup 6 script — packs `staging\` 1:1 into `{app}`, adds a Start-Menu (and optional desktop) shortcut, outputs `Qcast-Setup-<version>.exe`. |
| `README.md` | This document. |
| `.gitignore` | Ignores build outputs (`staging/`, `Output/`, stray `*.exe`). |

## Prerequisites (on the Windows build machine)

These are the same dependencies `deploy/setup-windows.ps1` provisions; you can run
that script once to get most of them, then add Inno Setup.

1. **Visual Studio 2022 Build Tools** with the **Desktop development with C++**
   workload (the MSVC C++ toolchain). Required by the Rust MSVC toolchain and
   cargo-c.
2. **GStreamer 1.26.x MSVC** — **both** the runtime and the development MSIs, for
   `x86_64`, installed with `ADDLOCAL=ALL` so every plugin (incl. `-bad` with
   `d3d11` + the WebRTC transport deps) is present. Download from
   <https://gstreamer.freedesktop.org/data/pkg/windows/>. The runtime MSI sets
   `GSTREAMER_1_0_ROOT_MSVC_X86_64` (default root `C:\gstreamer\1.0\msvc_x86_64\`).
3. **Rust (MSVC toolchain)** via rustup (`x86_64-pc-windows-msvc`).
4. **cargo-c** (`cargo install cargo-c`) — builds the C-ABI `gstrswebrtc.dll`.
5. **Inno Setup 6** — provides `ISCC.exe` (the command-line compiler). Download
   from <https://jrsoftware.org/isdl.php>.
6. A checkout of **gst-plugins-rs** (branch `0.15`, matching GStreamer 1.26) for the
   `gst-plugin-webrtc` build. `deploy/setup-windows.ps1` clones it to
   `%USERPROFILE%\.cache\qcast-build\gst-plugins-rs`; `gather-payload.ps1` defaults
   to that location (override with `-PluginsRsDir`).

## Build sequence

Run from the repo root in a PowerShell where `cargo` uses the MSVC toolchain (the
same environment `deploy/setup-windows.ps1` prepares — it sets `PKG_CONFIG_PATH`
for cargo-c and puts the GStreamer `bin` on `PATH`).

### 1. Assemble the payload

```powershell
deploy\windows\gather-payload.ps1
```

This:

- runs `cargo build --release -p qcast-sender` → `target\release\qcast-sender.exe`;
- runs `cargo cbuild --release -p gst-plugin-webrtc` in the gst-plugins-rs checkout
  and locates `gstrswebrtc.dll`;
- resolves the GStreamer root (from `-GstRoot`, else `GSTREAMER_1_0_ROOT_MSVC_X86_64`,
  else `C:\gstreamer\1.0\msvc_x86_64`);
- copies the runtime `bin\*.dll`, the curated plugin DLLs + `gstrswebrtc.dll`, and
  `gst-plugin-scanner.exe` into `deploy\windows\staging\` with this layout:

  ```
  staging\qcast-sender.exe
  staging\*.dll                                        (GStreamer runtime)
  staging\lib\gstreamer-1.0\*.dll                      (plugins + gstrswebrtc.dll)
  staging\libexec\gstreamer-1.0\gst-plugin-scanner.exe
  ```

It is idempotent (cleans + recreates `staging\`) and **warns loudly** about any
plugin DLL it could not find rather than silently skipping it.

Useful flags:

```powershell
# explicit GStreamer root
deploy\windows\gather-payload.ps1 -GstRoot "C:\gstreamer\1.0\msvc_x86_64"

# use already-built inputs, skip the cargo builds
deploy\windows\gather-payload.ps1 -SkipBuild `
  -ExePath   ...\target\release\qcast-sender.exe `
  -PluginDll ...\gstrswebrtc.dll
```

### 2. Compile the installer

```powershell
ISCC.exe deploy\windows\qcast.iss
```

Produces `deploy\windows\Output\Qcast-Setup-0.1.0.exe`. Override the version /
staging dir at compile time:

```powershell
ISCC.exe /DAppVersion=0.1.0 /DStagingDir="C:\path\to\staging" deploy\windows\qcast.iss
```

> Keep `AppVersion` in sync with `crates/qcast-sender/Cargo.toml` (`version = "0.1.0"`).

## Code signing (Authenticode) — a real step

Unsigned installers trip Windows SmartScreen's **"unknown publisher"** warning,
which is a scary first run. Authenticode-sign **both** the binary (before staging)
and the finished setup exe. This needs an Authenticode code-signing certificate
(OV or, to suppress the SmartScreen reputation prompt immediately, EV) from a CA.

```powershell
# 1. Sign the host binary BEFORE running gather-payload.ps1 (so the staged copy is signed):
signtool sign /tr http://timestamp.digicert.com /td sha256 /fd sha256 `
  /a target\release\qcast-sender.exe

# 2. Run gather-payload.ps1 + ISCC.exe as above.

# 3. Sign the produced installer:
signtool sign /tr http://timestamp.digicert.com /td sha256 /fd sha256 `
  /a deploy\windows\Output\Qcast-Setup-0.1.0.exe
```

(`signtool.exe` ships with the Windows SDK. `/tr` adds an RFC-3161 timestamp so the
signature stays valid after the cert expires. `/a` auto-selects the best cert in the
store; use `/f cert.pfx /p <pw>` for a file-based cert.) Inno Setup can also sign
automatically via a configured `SignTool` directive if you prefer signing in-pipeline.

## Test on a clean Windows 10/11 VM

Test on a VM with **nothing** Qcast-related installed (no GStreamer, no Rust):

1. Copy `Qcast-Setup-<version>.exe` to the VM and run it. As a per-user install it
   should **not** prompt for admin/UAC; it installs to
   `%LOCALAPPDATA%\Programs\Qcast`.
2. Launch **Qcast** from the Start Menu. The GUI should appear and show the **viewer
   access password**.
3. Start streaming the **test pattern** (run with `--source test`, or use the GUI),
   open the shown URL on a **phone** on the same network, enter the password, and
   confirm the test stream plays. This proves the bundled plugins (incl.
   `gstrswebrtc.dll`, `vpx`, the WebRTC transport set) and the in-binary TURN relay
   all loaded from `{app}` with no system GStreamer present.
4. For real desktop capture, run normally (no `--source test`) and confirm
   `d3d11screencapturesrc` capture works.

If a VM happens to have a **different** GStreamer version installed and you see
plugin/version conflicts, set `QCAST_BUNDLE=1` for the process and retry — that
makes `bundle.rs` clear `GST_PLUGIN_SYSTEM_PATH_1_0` so only the bundled plugins are
used. See the note below.

## Known open items (resolve when tested on real Windows)

- **`QCAST_BUNDLE=1` and the shortcut.** A Start-Menu `.lnk` cannot set process
  environment variables, so the installer cannot inject `QCAST_BUNDLE=1`. The
  bundled plugins are still found (bundle.rs auto-prepends the sibling
  `lib\gstreamer-1.0` to `GST_PLUGIN_PATH`), which is enough on a machine with **no**
  conflicting GStreamer. If a host GStreamer of a different version causes conflicts,
  `bundle.rs` only clears the system plugin path when `QCAST_BUNDLE=1`. The
  recommended robust fix — **deferred to the on-Windows test phase, not done in this
  authoring phase** — is a one-line app-side change: on Windows, treat "a sibling
  `lib\gstreamer-1.0` exists" as implying bundle mode (clear the system path even
  without the env var). A small `qcast.cmd` wrapper that sets `QCAST_BUNDLE=1` then
  `start`s the exe is a stop-gap alternative.
- **Runtime `bin` trimming.** `gather-payload.ps1` bundles the *entire* GStreamer
  runtime `bin\*.dll` for correctness. To shrink the installer, trace the real
  dependency closure with `Dependencies.exe` / `dumpbin /dependents` and stage only
  what's needed. Not required for a working installer.
- **First-run validation.** `gather-payload.ps1` / `qcast.iss` were authored on Linux
  and never executed on Windows. Verify the plugin DLL names match the installed
  GStreamer 1.26 (the script warns on any it can't find), and confirm `ISCC.exe`
  compiles `qcast.iss` cleanly.

## Deferred alternative: `.msi` via WiX

A WiX-authored `.msi` is a **deferred optional** alternative, better suited to
**enterprise / Group-Policy** deployment (silent install, machine-wide policy
push). It is harder to author than Inno Setup and is not needed for the standard
end-user download; revisit only if enterprise deployment is required. See
`deploy/PACKAGING.md` §3.
