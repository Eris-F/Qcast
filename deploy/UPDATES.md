# Qcast — In-app updates

Qcast uses `tauri-plugin-updater` to pull signed updates from GitHub Releases.
This doc captures the end-to-end flow so future releases are deterministic.

> Status (Phase 5): the wiring is in place but the signing key has **not** been
> generated yet. The Fedora dev box can't run `cargo tauri signer generate`
> (no `cargo-tauri` installed). The first release must be cut from the Windows
> VM after the one-time key-gen step below.

## How it fits together

* `src-tauri/tauri.conf.json`
  * `bundle.createUpdaterArtifacts: true` — tells `cargo tauri build` to
    produce a `.sig` file next to the NSIS installer.
  * `plugins.updater.pubkey` — the Tauri-formatted public key. The plugin
    verifies the `.sig` against this before installing anything.
  * `plugins.updater.endpoints` — points at the `latest.json` manifest we
    upload alongside each release tag.
* `src-tauri/src/updater.rs` — `check()` / `apply()` helpers that the
  Phase-4 IPC commands `check_for_updates` / `apply_update` call into.
* `.github/workflows/release.yml` — `windows-tauri-updater` job builds the
  installer, signs it with the key from the `TAURI_SIGNING_*` secrets, builds
  `latest.json`, and attaches all three files to the GitHub Release.
* `crates/qcast-sender/web-client/package.json` — declares the JS bindings
  (`@tauri-apps/plugin-updater`, `@tauri-apps/plugin-process`) so the
  renderer can drive the flow directly if we ever want a progress bar.

## One-time setup (must run from the Windows VM)

1. Install the Tauri CLI on the build host:
   ```powershell
   cargo install tauri-cli --version "^2"
   ```
2. Generate the keypair. The private file is written to `.local-secrets/`,
   which is `.gitignore`d:
   ```powershell
   mkdir .local-secrets -Force
   cargo tauri signer generate -w .local-secrets\qcast-updater.key.txt
   ```
   You'll be prompted for a password. **Pick a strong one and record it
   out-of-band** (1Password / printed copy). Losing either the key OR the
   password bricks auto-update for everyone who's already installed Qcast.
3. The command writes two files:
   * `.local-secrets/qcast-updater.key.txt`     - the encrypted private key
   * `.local-secrets/qcast-updater.key.txt.pub` - the public key (base64)
4. Copy the contents of the `.pub` file into
   `src-tauri/tauri.conf.json` -> `plugins.updater.pubkey`. It currently reads
   `"REPLACE_ME_ON_FIRST_RELEASE"`.
5. Add two repository secrets in GitHub (Settings -> Secrets -> Actions):
   * `TAURI_SIGNING_PRIVATE_KEY` — paste the entire contents of
     `.local-secrets/qcast-updater.key.txt` (multi-line, base64-ish blob)
   * `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password you chose in step 2
6. Commit the updated `tauri.conf.json` (with the real pubkey) **but not**
   the private key file. The `.gitignore` already excludes `.local-secrets/`.
7. Back up `.local-secrets/qcast-updater.key.txt` + password somewhere
   durable. This is the most important step on this page.

## Release flow

Once the one-time setup is done, every release is:

```bash
git tag v0.1.1
git push origin v0.1.1
```

GitHub Actions then:

1. Builds the Windows NSIS installer via `cargo tauri build`.
2. Signs it with the secrets above, producing `Qcast_0.1.1_x64-setup.exe.sig`.
3. Assembles a `latest.json` manifest:
   ```json
   {
     "version": "0.1.1",
     "notes": "See the GitHub Release for full notes.",
     "pub_date": "2026-05-28T12:34:56Z",
     "platforms": {
       "windows-x86_64": {
         "signature": "<contents of the .sig file>",
         "url": "https://github.com/Eris-F/Qcast/releases/download/v0.1.1/Qcast_0.1.1_x64-setup.exe"
       }
     }
   }
   ```
4. Attaches all three files (`*.exe`, `*.exe.sig`, `latest.json`) to the
   GitHub Release for tag `v0.1.1`.

The `endpoints` entry in `tauri.conf.json` points at
`releases/latest/download/latest.json`, which GitHub resolves to the manifest
on whichever release is currently marked "latest". Pre-release tags are
ignored by the `latest` alias, which is the behaviour we want.

## What the running client does

On every startup (and when the user clicks the "Check for updates" row in
Settings), the renderer calls `check_for_updates` -> `crate::updater::check`
-> `app.updater().check().await`. The plugin fetches the configured
`latest.json`, compares the `version` against the installed semver, and
returns an `Update` struct only if the remote is newer.

If the user clicks "Update now", the renderer calls `apply_update` ->
`crate::updater::apply`, which `download_and_install`s the new `.exe`,
verifies its `.sig` against the embedded pubkey, runs the NSIS installer in
silent / current-user mode, and calls `AppHandle::restart()`.

## Failure modes & follow-ups

* **No signature on the build.** Means `TAURI_SIGNING_PRIVATE_KEY` isn't
  set on the runner. The `Build latest.json manifest` step asserts the
  `.sig` exists and fails loudly.
* **Pubkey mismatch.** The plugin will refuse the install and emit a
  signature-verification error. Re-paste the `.pub` contents into
  `tauri.conf.json`; the two MUST come from the same `signer generate` run.
* **Lost private key.** Generate a new keypair, ship the new pubkey in
  the next release, and accept that already-installed clients will never
  auto-update again — they'll have to be reinstalled manually. There is
  no key-rotation flow in Tauri's updater today.
* **GStreamer staging in CI.** The `windows-tauri-updater` job currently has
  a placeholder for assembling `src-tauri/gst-runtime/`. Lift the real steps
  from `deploy/windows/gather-payload.ps1` or `deploy/WINDOWS_INSTALLER.md`
  before cutting a real signed release. See the `TODO(WINDOWS_INSTALLER.md)`
  comments in `.github/workflows/release.yml`.
