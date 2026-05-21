<#
  Qcast - Windows guided setup.

  Provisions every dependency Qcast needs on Windows and builds the host, mirroring
  deploy/setup-linux.sh. Intended to be run once on a fresh Windows 10/11 machine
  (or a build machine) from an elevated PowerShell. Safe to re-run (idempotent):
  each step checks whether it's already satisfied first.

  Unlike Linux, Windows needs NO TURN server install - Qcast's relay is built into
  the binary. Screen capture uses GStreamer's d3d11screencapturesrc (Desktop
  Duplication), bundled in the GStreamer runtime below.

  What it does:
    1. Installs Git if missing (winget, or the GitHub installer as fallback).
    2. Installs the MSVC C++ build tools if missing (VS 2022 Build Tools, VCTools
       workload) - required by the Rust MSVC toolchain and cargo-c. Unattended.
    3. Installs the GStreamer MSVC runtime + development MSIs (all plugins).
    4. Installs the Rust toolchain (rustup) if missing.
    5. Installs cargo-c (builds the C-ABI webrtcsink plugin from Rust).
    6. Builds the gst-plugins-rs webrtc plugin and drops it in the plugin dir
       (only if webrtcsink isn't already present).
    7. Builds the Qcast host (cargo build --release).
    8. Verifies every required GStreamer element is available.

  Git and the C++ build tools install automatically and unattended (no prompts);
  both are large-ish but run in the background while the script waits.

  Usage (elevated PowerShell, from the repo root):
      powershell -ExecutionPolicy Bypass -File deploy\setup-windows.ps1
      ...\setup-windows.ps1 -Verify          # only re-run verification
      ...\setup-windows.ps1 -GstVersion 1.26.2 -NoBuild

  NOTE: written on a Linux dev box and not yet executed on Windows - treat the
  first run as the validation. The structure intentionally fails loudly so any
  gap (a renamed MSI, a missing plugin) is obvious rather than a silent runtime bug.
#>
[CmdletBinding()]
param(
  [string]$GstVersion = "1.26.2",     # GStreamer release to install (override if needed)
  [string]$Arch       = "x86_64",
  [string]$PluginsRsRef = "0.15",     # gst-plugins-rs branch matching GStreamer 1.26
  [switch]$Verify,
  [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path "$PSScriptRoot\..").Path

function Step($m){ Write-Host "`n==> $m" -ForegroundColor Cyan }
function Info($m){ Write-Host "    $m" }
function Ok($m)  { Write-Host "    [ok] $m" -ForegroundColor Green }
function Warn($m){ Write-Host "    [!] $m"  -ForegroundColor Yellow }
function Die($m) { Write-Host "`nERROR: $m" -ForegroundColor Red; exit 1 }

# Retry a scriptblock up to 3 times (network installs flake).
function Retry([scriptblock]$Action){
  for($i=1; $i -le 3; $i++){
    try { & $Action; return } catch {
      if($i -eq 3){ throw }
      Warn "attempt $i failed: $($_.Exception.Message); retrying in 3s"; Start-Sleep 3
    }
  }
}

function Have-Command($name){ [bool](Get-Command $name -ErrorAction SilentlyContinue) }
function Have-Winget(){ Have-Command winget }

# Re-read PATH from the registry so binaries installed during this run (git,
# cargo, …) become callable without opening a new shell.
function Refresh-Path(){
  $machine = [Environment]::GetEnvironmentVariable("Path","Machine")
  $user    = [Environment]::GetEnvironmentVariable("Path","User")
  $env:Path = "$machine;$user"
}

function Have-Element($name){
  $gi = Get-Command gst-inspect-1.0.exe -ErrorAction SilentlyContinue
  if(-not $gi){ return $false }
  & $gi.Source $name 1>$null 2>$null
  return ($LASTEXITCODE -eq 0)
}

# Is the MSVC C++ toolchain (linker + Windows SDK) installed? Rust's MSVC
# toolchain and cargo-c/webrtcsink can't build without it. We detect it via
# vswhere (the VS Installer's locator) requiring the VC.Tools component, and
# fall back to a bare cl.exe-on-PATH check.
function Have-BuildTools(){
  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  if(Test-Path $vswhere){
    $found = & $vswhere -products * -latest `
      -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
      -property installationPath 2>$null
    if($found){ return $true }
  }
  return (Have-Command cl)
}

# GStreamer install root for the MSVC build (set by the MSI; we also set it so the
# current session can build/inspect without a reboot).
function Gst-Root(){
  $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 `
    ?? "C:\gstreamer\1.0\msvc_${Arch}\"
}

function Add-ToPath($dir){
  if(Test-Path $dir){
    if($env:PATH -notlike "*$dir*"){ $env:PATH = "$dir;$env:PATH" }
  }
}

# ---------------------------------------------------------------------------
# 0a. Git (needed to clone gst-plugins-rs for the webrtcsink build)
# ---------------------------------------------------------------------------
function Ensure-Git(){
  Step "Ensuring Git"
  if(Have-Command git){ Ok "git present: $(git --version)"; return }
  Info "git not found — installing automatically…"
  if(Have-Winget){
    Retry { winget install --id Git.Git -e --silent `
      --accept-package-agreements --accept-source-agreements }
  } else {
    # No winget (older Windows): pull the latest Git-for-Windows 64-bit installer
    # from the GitHub API and run it unattended.
    Info "winget unavailable — fetching Git for Windows from GitHub…"
    $rel = Invoke-RestMethod "https://api.github.com/repos/git-for-windows/git/releases/latest" `
      -Headers @{ "User-Agent" = "qcast-setup" }
    $asset = $rel.assets | Where-Object { $_.name -match '64-bit\.exe$' } | Select-Object -First 1
    if(-not $asset){ Die "could not locate a Git for Windows installer asset" }
    $exe = Join-Path $env:TEMP $asset.name
    Retry { Invoke-WebRequest $asset.browser_download_url -OutFile $exe -UseBasicParsing }
    Retry { Start-Process $exe -Wait -ArgumentList "/VERYSILENT /NORESTART /SP-" }
  }
  Refresh-Path
  if(-not (Have-Command git)){ Die "git still not on PATH after install (open a new shell and re-run)" }
  Ok "installed: $(git --version)"
}

# ---------------------------------------------------------------------------
# 0b. MSVC C++ build tools (Rust MSVC toolchain + cargo-c/webrtcsink need them)
# ---------------------------------------------------------------------------
function Ensure-BuildTools(){
  Step "Ensuring Visual C++ build tools (MSVC)"
  if(Have-BuildTools){ Ok "Visual C++ build tools present"; return }
  Info "not found — installing VS 2022 Build Tools (C++ workload) automatically."
  Info "this is a large download (a few GB) and runs unattended; please wait…"
  $vsArgs = "--quiet --wait --norestart --nocache " +
            "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  if(Have-Winget){
    Retry { winget install --id Microsoft.VisualStudio.2022.BuildTools -e --silent `
      --accept-package-agreements --accept-source-agreements --override "$vsArgs" }
  } else {
    # Stable bootstrapper URL — works without winget.
    $bt = Join-Path $env:TEMP "vs_BuildTools.exe"
    Retry { Invoke-WebRequest "https://aka.ms/vs/17/release/vs_BuildTools.exe" -OutFile $bt -UseBasicParsing }
    Retry { Start-Process $bt -Wait -ArgumentList $vsArgs }
  }
  Refresh-Path
  if(Have-BuildTools){ Ok "Visual C++ build tools installed" }
  else { Warn "build-tools install finished but VCTools wasn't detected — a reboot may be needed before building" }
}

# ---------------------------------------------------------------------------
# 1. GStreamer runtime + devel MSIs
# ---------------------------------------------------------------------------
function Install-GStreamer(){
  Step "Installing GStreamer $GstVersion (MSVC $Arch) runtime + devel"
  $root = Gst-Root
  if(Test-Path (Join-Path $root "bin\gst-inspect-1.0.exe")){
    Ok "GStreamer already installed at $root"
  } else {
    $base = "https://gstreamer.freedesktop.org/data/pkg/windows/$GstVersion/msvc"
    $msis = @(
      "gstreamer-1.0-msvc-$Arch-$GstVersion.msi",
      "gstreamer-1.0-devel-msvc-$Arch-$GstVersion.msi"
    )
    foreach($m in $msis){
      $url = "$base/$m"; $out = Join-Path $env:TEMP $m
      Info "downloading $m"
      Retry { Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing }
      Info "installing $m (all features)"
      # ADDLOCAL=ALL installs every plugin set (incl. -bad with d3d11 + webrtc deps).
      Retry { Start-Process msiexec.exe -Wait -ArgumentList `
        "/i `"$out`" /qn ADDLOCAL=ALL" }
    }
    Ok "GStreamer installed"
  }
  # Make this session aware of it.
  [Environment]::SetEnvironmentVariable("GSTREAMER_1_0_ROOT_MSVC_X86_64", $root, "User")
  $env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $root
  Add-ToPath (Join-Path $root "bin")
  # pkg-config files for building gstreamer-rs.
  $env:PKG_CONFIG_PATH = (Join-Path $root "lib\pkgconfig")
}

# ---------------------------------------------------------------------------
# 2. Rust toolchain
# ---------------------------------------------------------------------------
function Ensure-Rust(){
  Step "Ensuring a Rust toolchain (MSVC)"
  if(Get-Command cargo.exe -ErrorAction SilentlyContinue){
    Ok "cargo present: $(cargo --version)"; return
  }
  Info "installing Rust via rustup..."
  $ri = Join-Path $env:TEMP "rustup-init.exe"
  Retry { Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $ri -UseBasicParsing }
  Retry { Start-Process $ri -Wait -ArgumentList "-y --default-host x86_64-pc-windows-msvc" }
  Add-ToPath (Join-Path $env:USERPROFILE ".cargo\bin")
  if(-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)){ Die "cargo still not on PATH after rustup" }
  Ok "installed: $(cargo --version)"
}

function Ensure-CargoC(){
  Step "Ensuring cargo-c"
  cargo cbuild --help 1>$null 2>$null
  if($LASTEXITCODE -eq 0){ Ok "cargo-c present"; return }
  Info "installing cargo-c (cargo install)..."
  Retry { cargo install cargo-c }
  Ok "cargo-c installed"
}

# ---------------------------------------------------------------------------
# 3. webrtcsink (gst-plugins-rs) - build only if missing
# ---------------------------------------------------------------------------
function Install-WebRTCSink(){
  Step "Installing the webrtcsink streaming plugin"
  if(Have-Element "webrtcsink"){ Ok "webrtcsink already available"; return }

  $build = Join-Path $env:USERPROFILE ".cache\qcast-build"
  New-Item -ItemType Directory -Force -Path $build | Out-Null
  $src = Join-Path $build "gst-plugins-rs"
  if(Test-Path (Join-Path $src ".git")){
    Info "reusing clone at $src"
  } else {
    Retry { git clone --depth 1 --branch $PluginsRsRef `
      https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git $src }
  }

  Info "building gst-plugin-webrtc (cargo cbuild)..."
  Push-Location $src
  try { Retry { cargo cbuild --release -p gst-plugin-webrtc } } finally { Pop-Location }

  $dll = Get-ChildItem -Path (Join-Path $src "target") -Recurse -Filter "gstrswebrtc.dll" `
    -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
  if(-not $dll){ Die "build succeeded but gstrswebrtc.dll not found" }
  $pluginDir = Join-Path (Gst-Root) "lib\gstreamer-1.0"
  Copy-Item $dll.FullName $pluginDir -Force
  Ok "installed gstrswebrtc.dll -> $pluginDir"

  if(-not (Have-Element "webrtcsink")){ Die "webrtcsink still not found after install" }
  Ok "webrtcsink is now available"
}

# ---------------------------------------------------------------------------
# 4. Build Qcast
# ---------------------------------------------------------------------------
function Build-Qcast(){
  if($NoBuild){ Info "skipping cargo build (-NoBuild)"; return }
  Step "Building the Qcast host (release)"
  Push-Location $RepoRoot
  try { Retry { cargo build --release -p qcast-sender } } finally { Pop-Location }
  Ok "built: $RepoRoot\target\release\qcast-sender.exe"
}

# ---------------------------------------------------------------------------
# 5. Verify
# ---------------------------------------------------------------------------
function Verify-Install(){
  Step "Verifying the installation"
  $missing = $false
  foreach($el in @("webrtcsink","videoconvert","videoscale","vp8enc","rtpbin","nicesink","dtlsenc","srtpenc")){
    if(Have-Element $el){ Ok "element: $el" } else { Warn "MISSING element: $el"; $missing = $true }
  }
  if(Have-Element "d3d11screencapturesrc"){ Ok "capture: d3d11screencapturesrc" }
  else { Warn "MISSING capture: d3d11screencapturesrc (GStreamer -bad plugins)"; $missing = $true }

  $exe = Join-Path $RepoRoot "target\release\qcast-sender.exe"
  if(Test-Path $exe){ Ok "qcast-sender.exe present" }
  elseif(-not $NoBuild){ Warn "qcast-sender.exe missing"; $missing = $true }

  if($missing){ Die "verification found missing components (see [!] lines). Re-run after resolving them." }
  Write-Host "`n[ok] Qcast is ready." -ForegroundColor Green
  Write-Host "Run:  $RepoRoot\target\release\qcast-sender.exe"
}

# ---------------------------------------------------------------------------
if($Verify){ Verify-Install; exit 0 }
Ensure-Git
Ensure-BuildTools
Install-GStreamer
Ensure-Rust
Ensure-CargoC
Install-WebRTCSink
Build-Qcast
Verify-Install
