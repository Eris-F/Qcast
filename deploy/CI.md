# Qcast CI release pipeline

> **STATUS: DRAFT.** The workflow at `.github/workflows/release.yml` automates the
> two build recipes that were validated **locally** (`deploy/appimage/build-appimage.sh`
> on Fedora 43; `deploy/windows/gather-payload.ps1` + `qcast.iss` authored on Linux,
> never run on Windows). It has **not** been executed on GitHub Actions yet. Treat
> the first real run — ideally a `workflow_dispatch`, not a tag — as the validation
> pass and expect to iterate. None of the jobs were run by the author of this file
> (GitHub Actions cannot run locally).

## What triggers it

`.github/workflows/release.yml` runs on:

- **`push` of a tag matching `v*`** (e.g. `v0.1.0`) — builds both artifacts **and**
  attaches them to the GitHub Release for that tag.
- **`workflow_dispatch`** (Actions tab → *Run workflow*) — builds both artifacts and
  uploads them as **build artifacts only** (no Release upload, since there is no tag).

It deliberately does **not** run on every push or pull request, to avoid surprise
runs and wasted minutes. `permissions: contents: write` is set so the release step
can create/update the Release. `strategy.fail-fast: false` on each job means one OS
failing does not cancel the other.

## What each job produces

| Job | Runner | Output | How |
| --- | --- | --- | --- |
| `linux-appimage` | `ubuntu-latest` | `Qcast-x86_64.AppImage` | installs the GStreamer apt stack (mirrors `deploy/setup-linux.sh`), Rust + cargo-c, builds `gst-plugins-rs@0.15` (`gst-plugin-webrtc` → `libgstrswebrtc.so`; `gst-plugin-rtp` → `libgstrsrtp.so`, best-effort), then runs `deploy/appimage/build-appimage.sh` as-is. |
| `windows-installer` | `windows-latest` | `Qcast-Setup-<ver>.exe` | installs GStreamer 1.26 MSVC (runtime + devel) + Rust MSVC + cargo-c + Inno Setup, clones `gst-plugins-rs@0.15`, runs `deploy/windows/gather-payload.ps1` (builds the exe + `gstrswebrtc.dll` and stages the payload), then compiles `deploy/windows/qcast.iss` with `ISCC.exe`. |

Both are uploaded with `actions/upload-artifact@v4`; on a tag both are also attached
to the Release with `softprops/action-gh-release@v2`.

Actions used (pinned to major versions): `actions/checkout@v4`,
`dtolnay/rust-toolchain@stable`, `actions/upload-artifact@v4`,
`softprops/action-gh-release@v2`.

## Known first-run risks

1. **AppImage: Fedora-vs-Ubuntu toolchain.** `build-appimage.sh` was validated on
   Fedora 43 and bakes in Fedora-toolchain workarounds — `NO_STRIP`, a patchelf
   **DT_RELR repair pass**, and reduced codegen parallelism for a rustc/LLVM crash.
   It also defaults to Fedora paths (`/usr/lib64/gstreamer-1.0`,
   `/usr/libexec/gstreamer-1.0`). The workflow overrides `GST_SYS_PLUGINS` /
   `GST_SYS_HELPERS` to the Debian/Ubuntu multiarch dirs
   (`/usr/lib/x86_64-linux-gnu/gstreamer-1.0`, `…/gstreamer1.0/gstreamer-1.0`) and
   sanity-checks they exist before invoking the script. **But the rest of the
   script's Fedora assumptions (the patchelf-corruption repair, the
   `restore_from_system` lib paths it scans, `gst-plugin-scanner` location) are
   unverified on Ubuntu.** The script is intentionally **not** modified — fixing it
   blindly for Ubuntu risks breaking the validated Fedora path. If the AppImage job
   fails, adjust the script (or the workflow's env) for Ubuntu, or switch to a
   Fedora container, rather than guessing.

2. **Windows: GStreamer / Inno Setup install + plugin DLL names.** The Windows
   recipe was authored on Linux and **never run on Windows**. Unverified pieces:
   - the `choco install gstreamer / gstreamer-devel` packages and the `1.26.0`
     version pin (the MSI route from `gstreamer.freedesktop.org` with `ADDLOCAL=ALL`
     is the documented alternative — see `deploy/windows/README.md`);
   - whether `GSTREAMER_1_0_ROOT_MSVC_X86_64` is exported (the workflow pins it to
     `C:\gstreamer\1.0\msvc_x86_64` defensively);
   - the **plugin DLL names** `gather-payload.ps1` expects against the actually
     installed 1.26 runtime (the script *warns* on any it cannot find — read the
     job log for `MISSING plugin DLL(s)`);
   - the `ISCC.exe` path (`C:\Program Files (x86)\Inno Setup 6\ISCC.exe`).

3. **Code signing not wired.** Both artifacts ship **unsigned**. The Windows
   installer will trip SmartScreen's "unknown publisher" warning. Authenticode
   signing (binary + setup exe) is documented in `deploy/windows/README.md`
   ("Code signing") but needs a CA-issued cert and secrets, so it is a **TODO** —
   add `signtool` steps + a `secrets`-backed cert once a certificate exists. There
   is no AppImage signing either.

4. **Version sync.** `qcast.iss` defaults `AppVersion` to `0.1.0`. Keep it in sync
   with `crates/qcast-sender/Cargo.toml`, or pass `/DAppVersion=<ver>` to `ISCC.exe`
   (the workflow currently relies on the default — wire the tag name in once the job
   is proven).

## How to iterate

1. **Use `workflow_dispatch` first.** From the GitHub *Actions* tab, pick the
   `release` workflow → *Run workflow* on `master`. This exercises both jobs and
   uploads artifacts **without** creating a tag or a Release — the cheapest way to
   shake out the install steps. Download the artifacts and smoke-test them
   (AppImage on a clean Linux box; the installer on a clean Windows 10/11 VM per
   `deploy/windows/README.md`).
2. **Fix forward in small commits.** Each dispatch is independent; adjust the apt
   list, the GStreamer/Inno install, or the env overrides and re-dispatch.
3. **Only then cut a tag.** Once a dispatch run is green and the artifacts work,
   push a `v*` tag to produce the Release with both files attached. (Or create the
   GitHub Release through the UI/`gh`, which pushes the tag and triggers the same
   workflow — the release step updates that Release in place.)
4. **Wire code signing** before any public release (see risk #3).
