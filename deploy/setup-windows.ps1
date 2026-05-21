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
      ...\setup-windows.ps1 -GstVersion 1.26.11 -NoBuild

  NOTE: written on a Linux dev box and not yet executed on Windows - treat the
  first run as the validation. The structure intentionally fails loudly so any
  gap (a renamed MSI, a missing plugin) is obvious rather than a silent runtime bug.
#>
[CmdletBinding()]
param(
  [string]$GstVersion = "1.26.11",    # GStreamer release to install (override if needed; must exist under gstreamer.freedesktop.org/data/pkg/windows/)
  [string]$Arch       = "x86_64",
  [string]$PluginsRsRef = "0.15",     # gst-plugins-rs branch matching GStreamer 1.26
  [switch]$Verify,
  [switch]$NoBuild
)

$ErrorActionPreference = "Stop"
# Force TLS 1.2+ for every download in this session. Windows PowerShell 5.1
# defaults to SSL3/TLS1.0 on older builds, which freedesktop.org / github.com /
# aka.ms now refuse - so a download would fail with an opaque error. .NET enums
# differ across versions, so try Tls13|Tls12 and fall back to Tls12 alone.
try {
  [Net.ServicePointManager]::SecurityProtocol =
    [Net.SecurityProtocolType]::Tls13 -bor [Net.SecurityProtocolType]::Tls12
} catch {
  [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}
$RepoRoot = (Resolve-Path "$PSScriptRoot\..").Path
# Set true by Invoke-Process when an installer returns 3010/1641 (reboot needed).
$script:RebootPending = $false

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

# Run a native command (cargo, git, ...) and THROW on a nonzero exit code.
#
# Why: PowerShell only turns *cmdlet* errors into exceptions; a native exe's
# nonzero exit does NOT throw, even with $ErrorActionPreference="Stop". That
# means `Retry { cargo ... }` would run cargo exactly once (it never throws, so
# Retry sees success) and a transient network failure during `cargo build` /
# `git clone` would never be retried. Wrapping the call so it throws lets the
# surrounding Retry{} actually retry, and makes a real failure abort loudly.
function Invoke-Native([scriptblock]$Action){
  & $Action
  if($LASTEXITCODE -ne 0){ throw "command exited with code $LASTEXITCODE" }
}

# Run an installer/process and WAIT, then actually inspect its exit code.
#
# Why this exists: `Start-Process -Wait` (without -PassThru) discards the exit
# code, so a failed msiexec / bootstrapper would look like success. We capture
# it with -PassThru and throw on failure so the surrounding Retry{} can retry,
# and the script fails loudly instead of silently producing a broken install.
#
# 0    = success.
# 3010 = success but a reboot is required (common for MSVC build tools / some
#        MSIs). We treat it as success and surface a one-line notice; the caller
#        can warn if a reboot is needed before building.
# 1641 = success, reboot initiated (msiexec). Also treated as success.
function Invoke-Process([string]$FilePath, [string[]]$Arguments){
  $p = Start-Process -FilePath $FilePath -ArgumentList $Arguments -Wait -PassThru -NoNewWindow
  $code = $p.ExitCode
  if($code -eq 0){ return $code }
  if($code -eq 3010 -or $code -eq 1641){
    Warn "$([System.IO.Path]::GetFileName($FilePath)) exited $code (reboot required) - treating as success"
    $script:RebootPending = $true
    return $code
  }
  throw "$([System.IO.Path]::GetFileName($FilePath)) failed with exit code $code"
}

function Have-Command($name){ [bool](Get-Command $name -ErrorAction SilentlyContinue) }
function Have-Winget(){ Have-Command winget }

# Run winget and interpret its exit code. winget returns a grab-bag of nonzero
# codes that aren't real failures (already-installed, no-applicable-upgrade,
# reboot required). We treat those as success and only throw on genuine errors
# so the surrounding Retry{} can retry. The post-install Have-* re-check is the
# final arbiter regardless.
function Invoke-Winget([string[]]$Arguments){
  & winget @Arguments
  $code = $LASTEXITCODE
  # 0                = success
  # -1978335189 (0x8A15002B) = no applicable upgrade / already newest
  # -1978335212 (0x8A150014) = package already installed
  # 3010 / -2147023436       = reboot required
  $benign = @(0, -1978335189, -1978335212, 3010, -2147023436)
  if($benign -notcontains $code){
    throw "winget exited with code $code"
  }
  if($code -eq 3010 -or $code -eq -2147023436){ $script:RebootPending = $true }
}

# Re-read PATH from the registry so binaries installed during this run (git,
# cargo, ...) become callable without opening a new shell. Merge with the current
# process PATH instead of overwriting it, so process-only additions made earlier
# this run (e.g. the GStreamer bin dir, ~/.cargo\bin) survive the refresh. Dedup
# while preserving order.
function Refresh-Path(){
  $machine = [Environment]::GetEnvironmentVariable("Path","Machine")
  $user    = [Environment]::GetEnvironmentVariable("Path","User")
  $combined = @($machine, $user, $env:Path) -join ";"
  $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
  $parts = foreach($p in $combined.Split(";")){
    if($p -and $seen.Add($p)){ $p }
  }
  $env:Path = ($parts -join ";")
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

# Name of the arch-specific env var the GStreamer MSI sets (e.g.
# GSTREAMER_1_0_ROOT_MSVC_X86_64). Derived from $Arch so a non-default arch still
# resolves the right variable instead of always assuming x86_64.
function Gst-RootEnvName(){
  return "GSTREAMER_1_0_ROOT_MSVC_$($Arch.ToUpper())"
}

# GStreamer install root for the MSVC build (set by the MSI; we also set it so the
# current session can build/inspect without a reboot).
#
# NOTE: written without the `??` null-coalescing operator on purpose - `??` is
# PowerShell 7+ only and Windows ships Windows PowerShell 5.1 in-box, where `??`
# is a parse error. This if/else is equivalent and works on both.
function Gst-Root(){
  $envName = Gst-RootEnvName
  $fromEnv = [Environment]::GetEnvironmentVariable($envName)
  if($fromEnv){ return $fromEnv }
  return "C:\gstreamer\1.0\msvc_${Arch}\"
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
  Info "git not found - installing automatically..."
  if(Have-Winget){
    Retry { Invoke-Winget @("install","--id","Git.Git","-e","--silent",
      "--accept-package-agreements","--accept-source-agreements") }
  } else {
    # No winget (older Windows): pull the latest Git-for-Windows 64-bit installer
    # from the GitHub API and run it unattended.
    Info "winget unavailable - fetching Git for Windows from GitHub..."
    $rel = Invoke-RestMethod "https://api.github.com/repos/git-for-windows/git/releases/latest" `
      -Headers @{ "User-Agent" = "qcast-setup" }
    $asset = $rel.assets | Where-Object { $_.name -match '64-bit\.exe$' } | Select-Object -First 1
    if(-not $asset){ Die "could not locate a Git for Windows installer asset" }
    $exe = Join-Path $env:TEMP $asset.name
    Retry { Invoke-WebRequest $asset.browser_download_url -OutFile $exe -UseBasicParsing }
    Retry { Invoke-Process $exe @("/VERYSILENT","/NORESTART","/SP-") }
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
  Info "not found - installing VS 2022 Build Tools (C++ workload) automatically."
  Info "this is a large download (a few GB) and runs unattended; please wait..."
  # Kept as both a single override string (winget) and an arg array (bootstrapper).
  $vsArgList = @("--quiet","--wait","--norestart","--nocache",
                 "--add","Microsoft.VisualStudio.Workload.VCTools","--includeRecommended")
  $vsArgs = $vsArgList -join " "
  if(Have-Winget){
    # winget exit codes are messy (e.g. 0x8A15002B = "no applicable upgrade").
    # Run it, then rely on the Have-BuildTools re-check below as the real guard
    # rather than trusting winget's return code.
    Retry { Invoke-Winget @("install","--id","Microsoft.VisualStudio.2022.BuildTools",
      "-e","--silent","--accept-package-agreements","--accept-source-agreements",
      "--override",$vsArgs) }
  } else {
    # Stable bootstrapper URL - works without winget.
    $bt = Join-Path $env:TEMP "vs_BuildTools.exe"
    Retry { Invoke-WebRequest "https://aka.ms/vs/17/release/vs_BuildTools.exe" -OutFile $bt -UseBasicParsing }
    Retry { Invoke-Process $bt $vsArgList }
  }
  Refresh-Path
  if(Have-BuildTools){ Ok "Visual C++ build tools installed" }
  else { Warn "build-tools install finished but VCTools wasn't detected - a reboot may be needed before building (then re-run this script)" }
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
      try {
        # A 404/500 throws here (Invoke-WebRequest is a cmdlet) and is caught below.
        Retry { Invoke-WebRequest -Uri $url -OutFile $out -UseBasicParsing }
      } catch {
        Die ("could not download $m from $url`n" +
             "    Check that GStreamer $GstVersion exists for $Arch (a 404 means the version is wrong).`n" +
             "    Pick a published version from https://gstreamer.freedesktop.org/data/pkg/windows/`n" +
             "    and pass it, e.g.:  -GstVersion 1.26.11")
      }
      # Guard against a truncated/HTML-error download silently feeding msiexec a
      # bogus file (the runtime MSI is tens of MB; anything tiny is wrong).
      $size = (Get-Item $out -ErrorAction SilentlyContinue).Length
      if(-not $size -or $size -lt 1MB){
        Die "downloaded $m looks invalid ($([math]::Round(($size/1KB),1)) KB) - the URL may have returned an error page. URL: $url"
      }
      Info "installing $m (all features)"
      # ADDLOCAL=ALL installs every plugin set (incl. -bad with d3d11 + webrtc deps).
      # /i = install, /qn = silent, /norestart so a reboot-required MSI returns
      # 3010 (handled as success) instead of rebooting mid-setup.
      $log = Join-Path $env:TEMP "$m.install.log"
      Retry { Invoke-Process "msiexec.exe" @("/i","$out","/qn","/norestart","/l*v","$log","ADDLOCAL=ALL") }
    }
    Ok "GStreamer installed"
  }
  # Make this session aware of it (under the arch-specific env var the MSI uses).
  $envName = Gst-RootEnvName
  [Environment]::SetEnvironmentVariable($envName, $root, "User")
  Set-Item -Path "Env:$envName" -Value $root
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
  # Official Windows rustup bootstrapper, fetched over HTTPS from the official
  # host. rustup publishes no checksum for this shim, so we rely on TLS.
  $ri = Join-Path $env:TEMP "rustup-init.exe"
  Retry { Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $ri -UseBasicParsing }
  Retry { Invoke-Process $ri @("-y","--default-host","x86_64-pc-windows-msvc") }
  Add-ToPath (Join-Path $env:USERPROFILE ".cargo\bin")
  if(-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)){
    Die "cargo still not on PATH after rustup (open a new shell, or add %USERPROFILE%\.cargo\bin to PATH, then re-run)"
  }
  Ok "installed: $(cargo --version)"
}

function Ensure-CargoC(){
  Step "Ensuring cargo-c"
  cargo cbuild --help 1>$null 2>$null
  if($LASTEXITCODE -eq 0){ Ok "cargo-c present"; return }
  Info "installing cargo-c (cargo install)..."
  try { Retry { Invoke-Native { cargo install cargo-c } } }
  catch { Die "cargo install cargo-c failed: $($_.Exception.Message) - see the cargo output above" }
  # Belt and braces: confirm the subcommand actually landed and works.
  cargo cbuild --help 1>$null 2>$null
  if($LASTEXITCODE -ne 0){ Die "cargo install cargo-c did not produce a working 'cargo cbuild' - see the cargo output above" }
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
    Info "reusing clone at $src - refreshing to $PluginsRsRef"
    try {
      Retry { Invoke-Native { git -C $src fetch --depth 1 origin $PluginsRsRef } }
      Invoke-Native { git -C $src checkout -q FETCH_HEAD }
    } catch {
      Warn "could not refresh clone ($($_.Exception.Message)); building the existing checkout"
    }
  } else {
    try {
      Retry { Invoke-Native { git clone --depth 1 --branch $PluginsRsRef `
        https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git $src } }
    } catch {
      Die ("could not clone gst-plugins-rs (branch $PluginsRsRef) from gitlab.freedesktop.org: " +
           "$($_.Exception.Message)`n    Check network/HTTPS access and that the branch exists.")
    }
  }

  Info "building gst-plugin-webrtc (cargo cbuild)..."
  # cargo-c needs to find GStreamer's .pc files; Install-GStreamer set
  # PKG_CONFIG_PATH for this session. Surface a clear hint if the build fails.
  Push-Location $src
  try {
    Retry { Invoke-Native { cargo cbuild --release -p gst-plugin-webrtc } }
  } catch {
    Die ("cargo cbuild of gst-plugin-webrtc failed: $($_.Exception.Message)`n" +
         "    Common causes: MSVC build tools missing/not on PATH, or pkg-config can't`n" +
         "    find GStreamer (PKG_CONFIG_PATH=$env:PKG_CONFIG_PATH). See the cargo output above.")
  } finally { Pop-Location }

  $dll = Get-ChildItem -Path (Join-Path $src "target") -Recurse -Filter "gstrswebrtc.dll" `
    -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
  if(-not $dll){ Die "cargo cbuild reported success but gstrswebrtc.dll was not found under $src\target" }
  $pluginDir = Join-Path (Gst-Root) "lib\gstreamer-1.0"
  New-Item -ItemType Directory -Force -Path $pluginDir | Out-Null
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
  try {
    # Invoke-Native makes cargo's nonzero exit throw so Retry actually retries a
    # transient (e.g. crates.io) failure; a persistent failure aborts loudly.
    try { Retry { Invoke-Native { cargo build --release -p qcast-sender } } }
    catch { Die "cargo build of qcast-sender failed: $($_.Exception.Message) - see the cargo output above" }
  } finally { Pop-Location }
  Ok "built: $RepoRoot\target\release\qcast-sender.exe"
}

# ---------------------------------------------------------------------------
# 5. Verify
# ---------------------------------------------------------------------------
function Verify-Install(){
  Step "Verifying the installation"
  if(-not (Get-Command gst-inspect-1.0.exe -ErrorAction SilentlyContinue)){
    # Make sure the current session can find GStreamer even on a --verify-only run.
    Add-ToPath (Join-Path (Gst-Root) "bin")
  }
  if(-not (Get-Command gst-inspect-1.0.exe -ErrorAction SilentlyContinue)){
    Die ("gst-inspect-1.0.exe is not on PATH - GStreamer isn't installed or its bin dir`n" +
         "    isn't in PATH. Expected at $((Join-Path (Gst-Root) 'bin')). Open a new shell or re-run.")
  }
  $missing = $false
  # webrtcbin is included because the host's preflight (missing_webrtc_support)
  # requires it alongside webrtcsink.
  foreach($el in @("webrtcsink","webrtcbin","videoconvert","videoscale","vp8enc","rtpbin","nicesink","dtlsenc","srtpenc")){
    if(Have-Element $el){ Ok "element: $el" } else { Warn "MISSING element: $el"; $missing = $true }
  }
  # Accept either Windows capture source: Qcast prefers d3d11screencapturesrc
  # (Desktop Duplication) but falls back to wgcsrc (Windows Graphics Capture),
  # exactly like pick_screen_source() in the Rust core - so don't fail if only
  # one of them is present.
  $cap = $null
  if(Have-Element "d3d11screencapturesrc"){ $cap = "d3d11screencapturesrc" }
  elseif(Have-Element "wgcsrc"){ $cap = "wgcsrc" }
  if($cap){ Ok "capture: $cap" }
  else { Warn "MISSING capture source (d3d11screencapturesrc/wgcsrc - GStreamer -bad plugins)"; $missing = $true }

  # Accept a release OR debug build (a developer may build debug); on a
  # --verify-only run, not having built yet is expected, so it's a soft note.
  $rel = Join-Path $RepoRoot "target\release\qcast-sender.exe"
  $dbg = Join-Path $RepoRoot "target\debug\qcast-sender.exe"
  $built = $null
  if(Test-Path $rel){ Ok "qcast-sender.exe present (release)"; $built = $rel }
  elseif(Test-Path $dbg){ Ok "qcast-sender.exe present (debug)"; $built = $dbg }
  elseif($Verify){ Warn "qcast-sender.exe not built yet (run without -Verify, or 'cargo build --release -p qcast-sender')" }
  elseif(-not $NoBuild){ Warn "qcast-sender.exe missing after build"; $missing = $true }

  if($missing){ Die "verification found missing components (see [!] lines). Re-run after resolving them, or check the GStreamer install." }
  if($script:RebootPending){ Warn "an installer requested a reboot - reboot before running Qcast if anything misbehaves." }
  Write-Host "`n[ok] Qcast is ready." -ForegroundColor Green
  if($built){ Write-Host "Run:  $built" }
  else { Write-Host "Run:  $rel" }
}

# True if this process is running elevated (Administrator). System-wide MSI
# installs (GStreamer, VS Build Tools) and winget package installs need it.
function Test-Admin(){
  try {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = New-Object Security.Principal.WindowsPrincipal($id)
    return $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
  } catch { return $false }
}

# ---------------------------------------------------------------------------
# -Verify only inspects the environment, so it needs no elevation. The install
# path does: fail early with a clear message instead of dying cryptically deep
# inside msiexec/winget when a non-elevated run hits the first system install.
if($Verify){ Verify-Install; exit 0 }
if(-not (Test-Admin)){
  Die ("this script must run from an ELEVATED PowerShell (Run as Administrator) to install`n" +
       "    GStreamer, the MSVC build tools, and Git system-wide.`n" +
       "    Right-click PowerShell -> 'Run as administrator', then re-run:`n" +
       "        powershell -ExecutionPolicy Bypass -File deploy\setup-windows.ps1`n" +
       "    (Use -Verify to only check an existing install; that needs no elevation.)")
}
Ensure-Git
Ensure-BuildTools
Install-GStreamer
Ensure-Rust
Ensure-CargoC
Install-WebRTCSink
Build-Qcast
Verify-Install
