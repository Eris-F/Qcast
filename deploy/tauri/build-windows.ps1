<#
  build-windows.ps1 — build the Qcast Tauri NSIS installer on Windows in one command.

  Pipeline:
    1. Stage the bundled GStreamer runtime (flat DLLs + curated plugins + scanner)
       into <TauriDir>\gst-runtime\{bin,lib,libexec}, the layout tauri.conf.json's
       bundle.resources maps into the installer.
    2. cargo tauri build  →  the NSIS -setup.exe.

  The installer-hooks.nsh relocates the flat gst-runtime\bin DLLs next to the exe at
  install time (WINDOWS_INSTALLER.md risk #1). Plugins + scanner stay under
  resources\, where bundle.rs's resources\ candidate path finds them.

  Prereqs (see deploy\tauri\README.md): GStreamer 1.26 MSVC (runtime+devel,
  ADDLOCAL=ALL), Rust MSVC, cargo-c, cargo-tauri, and the Tauri app crate created via
  `cargo tauri init` (default TauriDir = src-tauri).

  NOTE: authored on the Linux dev box; this is the first-run validation on Windows.
  Run from the repo root:  deploy\tauri\build-windows.ps1
#>
[CmdletBinding()]
param(
  # Where `cargo tauri init` created the app (holds tauri.conf.json + src\).
  [string]$TauriDir = "src-tauri",
  # GStreamer MSVC root; passed to gather-payload.ps1 (defaults to the env var / the
  # conventional C:\gstreamer\1.0\msvc_x86_64).
  [string]$GstRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$AppDir   = Join-Path $RepoRoot $TauriDir
$Staging  = Join-Path $AppDir "gst-runtime"

if (-not (Test-Path -LiteralPath (Join-Path $AppDir "tauri.conf.json"))) {
  throw "No tauri.conf.json under $AppDir — run `cargo tauri init` first (see deploy\tauri\README.md), and copy deploy\tauri\tauri.conf.json + installer-hooks.nsh into it."
}

Write-Host "==> Staging GStreamer payload into $Staging" -ForegroundColor Cyan
# Reuse the curated plugin list from the Inno path so there is ONE source of truth.
$gather = Join-Path $RepoRoot "deploy\windows\gather-payload.ps1"
$gatherArgs = @{ StagingDir = $Staging }
if ($GstRoot) { $gatherArgs["GstRoot"] = $GstRoot }
& $gather @gatherArgs

# gather-payload lays out: <staging>\*.dll (flat runtime) + lib\gstreamer-1.0 (plugins)
# + libexec\... (scanner) + qcast-sender.exe. For the Tauri bundle we want the flat
# runtime DLLs under bin\ (so installer-hooks.nsh can relocate them) and no stray exe
# (Tauri builds its own).
Write-Host "==> Reshaping staging into bin\ + lib\ + libexec\" -ForegroundColor Cyan
$Bin = Join-Path $Staging "bin"
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
Get-ChildItem -Path $Staging -Filter *.dll -File | Move-Item -Destination $Bin -Force
Remove-Item -Path (Join-Path $Staging "qcast-sender.exe") -Force -ErrorAction SilentlyContinue

Write-Host "==> cargo tauri build" -ForegroundColor Cyan
Push-Location $AppDir
try {
  cargo tauri build
  if ($LASTEXITCODE -ne 0) { throw "cargo tauri build exited with code $LASTEXITCODE" }
} finally {
  Pop-Location
}

$setup = Get-ChildItem -Path (Join-Path $AppDir "target\release\bundle\nsis") -Filter "*-setup.exe" -File -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($setup) {
  Write-Host "`nDone. Installer: $($setup.FullName)" -ForegroundColor Green
} else {
  Write-Host "`nBuild finished but no -setup.exe found under target\release\bundle\nsis — check the tauri build output." -ForegroundColor Yellow
}
