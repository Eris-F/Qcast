<#
  gather-payload.ps1 - assemble the Qcast Windows installer payload.

  Runs on a WINDOWS build machine (or a CI windows runner). Stages everything the
  Inno Setup installer (qcast.iss) ships into a single directory laid out EXACTLY
  as it must land under {app} on the user's machine:

      <staging>\qcast-sender.exe                              the GUI host binary
      <staging>\*.dll                                         GStreamer runtime DLLs
      <staging>\lib\gstreamer-1.0\*.dll                       GStreamer plugins + ours
      <staging>\libexec\gstreamer-1.0\gst-plugin-scanner.exe  the plugin scanner

  This layout matches crates\qcast-sender\src\bundle.rs, which - BEFORE gst::init() -
  resolves bundled plugins at "<exedir>\lib\gstreamer-1.0" and the scanner at
  "<exedir>\libexec\gstreamer-1.0\gst-plugin-scanner.exe", and prepends them to
  GST_PLUGIN_PATH / sets GST_PLUGIN_SCANNER. So no env var needs to be set by the
  installer for the bundled plugins to be found.

  The plugin set mirrors the curated Linux AppImage list in
  deploy\appimage\build-appimage.sh (coreelements, videoconvertscale, videorate,
  vpx, encoding, debugutilsbad/-bad helpers, rtp, rtpmanager, nice, dtls, srtp,
  sctp, app, typefindfunctions, videotestsrc, audio*, opus, autodetect, playback,
  + our gstrswebrtc.dll), MINUS the Linux-only capture plugins (pipewire, ximagesrc)
  PLUS the Windows-only capture plugin d3d11 (d3d11screencapturesrc ! d3d11download,
  see capture.rs).

  Re-runnable & idempotent: cleans and recreates the staging dir each run. Warns
  (does not silently skip) about any plugin DLL it cannot find, and prints a clear
  summary of what it staged.

  NOTE: authored on a Linux dev box and NOT yet executed on Windows - treat the
  first run as validation. It fails loudly on missing inputs so any gap (a renamed
  plugin DLL, a wrong root) is obvious rather than a silent runtime bug.

  Usage (from the repo root, in a PowerShell that can run cargo with the MSVC
  toolchain - e.g. the same shell deploy\setup-windows.ps1 prepared):

      deploy\windows\gather-payload.ps1
      deploy\windows\gather-payload.ps1 -GstRoot "C:\gstreamer\1.0\msvc_x86_64"
      deploy\windows\gather-payload.ps1 -ExePath ...\qcast-sender.exe -PluginDll ...\gstrswebrtc.dll -SkipBuild
#>
[CmdletBinding()]
param(
  # GStreamer MSVC install root. Defaults to the env var the MSI sets
  # (GSTREAMER_1_0_ROOT_MSVC_X86_64), then the conventional install path.
  [string]$GstRoot,

  # Architecture suffix used for the env-var name and default root path.
  [string]$Arch = "x86_64",

  # Prebuilt inputs. If omitted, they are built (unless -SkipBuild) and located.
  [string]$ExePath,      # ...\target\release\qcast-sender.exe
  [string]$PluginDll,    # ...\gstrswebrtc.dll (from cargo cbuild of gst-plugin-webrtc)

  # Where the gst-plugins-rs checkout lives, so we can build/find gstrswebrtc.dll.
  # Mirrors deploy\setup-windows.ps1's clone location.
  [string]$PluginsRsDir = (Join-Path $env:USERPROFILE ".cache\qcast-build\gst-plugins-rs"),

  # Output staging dir (cleaned + recreated each run).
  [string]$StagingDir,

  # Skip the cargo build steps (use already-built ExePath / PluginDll).
  [switch]$SkipBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------------------
# Paths + logging
# ---------------------------------------------------------------------------
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "..\..")).Path
if (-not $StagingDir) { $StagingDir = Join-Path $ScriptDir "staging" }

function Step($m) { Write-Host "`n==> $m" -ForegroundColor Cyan }
function Info($m) { Write-Host "    $m" }
function Ok($m)   { Write-Host "    [ok] $m" -ForegroundColor Green }
function Warn($m) { Write-Host "    [!] $m"  -ForegroundColor Yellow }
function Die($m)  { Write-Host "`nERROR: $m" -ForegroundColor Red; exit 1 }

# Run a native command and THROW on nonzero exit (PowerShell does not do this for
# native exes even with $ErrorActionPreference="Stop").
function Invoke-Native([scriptblock]$Action) {
  & $Action
  if ($LASTEXITCODE -ne 0) { throw "command exited with code $LASTEXITCODE" }
}

# ---------------------------------------------------------------------------
# Resolve the GStreamer MSVC root.
# ---------------------------------------------------------------------------
function Resolve-GstRoot() {
  if ($GstRoot) { return $GstRoot.TrimEnd('\') }
  $envName = "GSTREAMER_1_0_ROOT_MSVC_$($Arch.ToUpper())"
  $fromEnv = [Environment]::GetEnvironmentVariable($envName)
  if ($fromEnv) { return $fromEnv.TrimEnd('\') }
  return "C:\gstreamer\1.0\msvc_$Arch"
}

# ---------------------------------------------------------------------------
# The curated plugin set. Cross-referenced against the Linux AppImage list in
# deploy\appimage\build-appimage.sh. Windows DLLs are named "gst<name>.dll"
# (no "lib" prefix, ".dll" suffix) vs Linux "libgst<name>.so".
#
#   Linux AppImage              Windows DLL                why
#   -----------------------     -----------------------    --------------------------------
#   libgstcoreelements.so       gstcoreelements.dll        queue/capsfilter/tee/identity
#   libgstvideoconvertscale.so  gstvideoconvertscale.dll   videoconvert + videoscale (1.26)
#   libgstvideorate.so          gstvideorate.dll           webrtcsink codec-discovery
#   libgstvpx.so                gstvpx.dll                  vp8enc/vp8dec (preferred codec)
#   libgstencoding.so           gstencoding.dll            encodebin (webrtcsink encode path)
#   libgstdebugutilsbad.so      gstdebugutilsbad.dll       errorignore (webrtcsink discovery)
#   libgstwebrtc.so             gstwebrtc.dll              webrtcbin (webrtcsink creates it - CRITICAL)
#   libgstrsrtp.so              gstrsrtp.dll               rtpgccbwe (Google congestion control - reliability)
#   libgstrtp.so                gstrtp.dll                 rtp payloaders
#   libgstrtpmanager.so         gstrtpmanager.dll          rtpbin
#   libgstnice.so               gstnice.dll                ICE
#   libgstdtls.so               gstdtls.dll                DTLS
#   libgstsrtp.so               gstsrtp.dll                SRTP
#   libgstsctp.so               gstsctp.dll                data channel
#   libgstapp.so                gstapp.dll                 appsrc/appsink (webrtcsink internals)
#   libgsttypefindfunctions.so  gsttypefindfunctions.dll   caps negotiation
#   libgstvideotestsrc.so       gstvideotestsrc.dll        --source test
#   libgstaudiotestsrc.so       gstaudiotestsrc.dll        optional audio track
#   libgstaudioconvert.so       gstaudioconvert.dll        audio convert (if negotiated)
#   libgstaudioresample.so      gstaudioresample.dll       audio resample (if negotiated)
#   libgstopus.so               gstopus.dll                opus audio (if negotiated)
#   libgstautodetect.so         gstautodetect.dll          webrtcsink internal helper
#   libgstplayback.so           gstplayback.dll            webrtcsink internal helper
#   (Linux-only, EXCLUDED)      -                          libgstpipewire.so, libgstximagesrc.so
#   (Windows-only, ADDED)       gstd3d11.dll               d3d11screencapturesrc + d3d11download
# ---------------------------------------------------------------------------
$Plugins = @(
  "gstcoreelements.dll",
  "gstvideoconvertscale.dll",
  "gstvideorate.dll",
  "gstvpx.dll",
  "gstencoding.dll",
  "gstdebugutilsbad.dll",
  "gstwebrtc.dll",        # webrtcbin - webrtcsink creates this internally (CRITICAL)
  "gstrsrtp.dll",         # rtpgccbwe - Google congestion control (reliability)
  "gstrtp.dll",
  "gstrtpmanager.dll",
  "gstnice.dll",
  "gstdtls.dll",
  "gstsrtp.dll",
  "gstsctp.dll",
  "gstapp.dll",
  "gsttypefindfunctions.dll",
  "gstvideotestsrc.dll",
  "gstaudiotestsrc.dll",
  "gstaudioconvert.dll",
  "gstaudioresample.dll",
  "gstopus.dll",
  "gstautodetect.dll",
  "gstplayback.dll",
  # Windows-only capture (see capture.rs: d3d11screencapturesrc ! d3d11download).
  "gstd3d11.dll"
)

# ---------------------------------------------------------------------------
# 1. Build (or accept) the binary + our plugin.
# ---------------------------------------------------------------------------
Step "Building / locating qcast-sender.exe + gstrswebrtc.dll"

if (-not $SkipBuild) {
  Info "cargo build --release -p qcast-sender"
  Push-Location $RepoRoot
  try { Invoke-Native { cargo build --release -p qcast-sender } }
  finally { Pop-Location }
}

if (-not $ExePath) {
  $ExePath = Join-Path $RepoRoot "target\release\qcast-sender.exe"
}
if (-not (Test-Path -LiteralPath $ExePath)) {
  Die "qcast-sender.exe not found at: $ExePath (build it or pass -ExePath)"
}
Ok "exe: $ExePath"

if (-not $SkipBuild) {
  if (-not (Test-Path -LiteralPath (Join-Path $PluginsRsDir "Cargo.toml"))) {
    Die ("gst-plugins-rs checkout not found at: $PluginsRsDir`n" +
         "    Clone it (see deploy\setup-windows.ps1) or pass -PluginsRsDir / -PluginDll + -SkipBuild.")
  }
  Info "cargo cbuild --release -p gst-plugin-webrtc (in $PluginsRsDir)"
  Push-Location $PluginsRsDir
  try { Invoke-Native { cargo cbuild --release -p gst-plugin-webrtc } }
  finally { Pop-Location }
}

if (-not $PluginDll) {
  # Locate the freshest gstrswebrtc.dll under the gst-plugins-rs target tree,
  # same way deploy\setup-windows.ps1 does.
  if (Test-Path -LiteralPath (Join-Path $PluginsRsDir "target")) {
    $found = Get-ChildItem -Path (Join-Path $PluginsRsDir "target") -Recurse -Filter "gstrswebrtc.dll" `
      -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($found) { $PluginDll = $found.FullName }
  }
}
if (-not $PluginDll -or -not (Test-Path -LiteralPath $PluginDll)) {
  Die ("gstrswebrtc.dll not found (looked under $PluginsRsDir\target).`n" +
       "    Build gst-plugin-webrtc with cargo cbuild, or pass -PluginDll.")
}
Ok "our plugin: $PluginDll"

# ---------------------------------------------------------------------------
# 2. Resolve + validate the GStreamer MSVC runtime layout.
# ---------------------------------------------------------------------------
Step "Resolving GStreamer MSVC runtime"
$root = Resolve-GstRoot
Info "GStreamer root: $root"
$gstBin     = Join-Path $root "bin"
$gstPlugins = Join-Path $root "lib\gstreamer-1.0"
$gstScanner = Join-Path $root "libexec\gstreamer-1.0\gst-plugin-scanner.exe"
if (-not (Test-Path -LiteralPath $gstBin))     { Die "GStreamer bin dir not found: $gstBin (is the MSVC runtime installed?)" }
if (-not (Test-Path -LiteralPath $gstPlugins)) { Die "GStreamer plugin dir not found: $gstPlugins" }
if (-not (Test-Path -LiteralPath $gstScanner)) { Die "gst-plugin-scanner.exe not found: $gstScanner" }
Ok "runtime bin / plugins / scanner all present"

# ---------------------------------------------------------------------------
# 3. Clean + create the staging dir (idempotent).
# ---------------------------------------------------------------------------
Step "Staging payload at $StagingDir"
if (Test-Path -LiteralPath $StagingDir) { Remove-Item -LiteralPath $StagingDir -Recurse -Force }
$stageLib     = Join-Path $StagingDir "lib\gstreamer-1.0"
$stageLibexec = Join-Path $StagingDir "libexec\gstreamer-1.0"
New-Item -ItemType Directory -Force -Path $StagingDir   | Out-Null
New-Item -ItemType Directory -Force -Path $stageLib     | Out-Null
New-Item -ItemType Directory -Force -Path $stageLibexec | Out-Null

# 3a. The host binary at the staging root (-> {app}\qcast-sender.exe).
Copy-Item -LiteralPath $ExePath -Destination (Join-Path $StagingDir "qcast-sender.exe") -Force
Ok "staged qcast-sender.exe"

# 3b. The full GStreamer runtime bin\*.dll at the staging root (-> {app}\*.dll).
# Bundle the whole runtime bin first; trim later with a dependency tracer
# (Dependencies.exe / dumpbin /dependents) if installer size matters.
$runtimeDlls = Get-ChildItem -Path $gstBin -Filter "*.dll" -File
foreach ($d in $runtimeDlls) {
  Copy-Item -LiteralPath $d.FullName -Destination (Join-Path $StagingDir $d.Name) -Force
}
Ok ("staged {0} GStreamer runtime DLL(s) from bin\" -f $runtimeDlls.Count)

# 3c. The curated plugin set + our plugin (-> {app}\lib\gstreamer-1.0\).
$staged = 0
$missing = @()
foreach ($p in $Plugins) {
  $srcPlugin = Join-Path $gstPlugins $p
  if (Test-Path -LiteralPath $srcPlugin) {
    Copy-Item -LiteralPath $srcPlugin -Destination (Join-Path $stageLib $p) -Force
    $staged++
  } else {
    $missing += $p
    Warn "plugin not found, NOT staged: $p (looked in $gstPlugins)"
  }
}
# Our webrtcsink plugin always comes from the cargo-c build, not the GStreamer root.
Copy-Item -LiteralPath $PluginDll -Destination (Join-Path $stageLib "gstrswebrtc.dll") -Force
$staged++
Ok ("staged {0} plugin DLL(s) into lib\gstreamer-1.0 (incl. gstrswebrtc.dll)" -f $staged)

# 3d. The plugin scanner (-> {app}\libexec\gstreamer-1.0\gst-plugin-scanner.exe).
Copy-Item -LiteralPath $gstScanner -Destination (Join-Path $stageLibexec "gst-plugin-scanner.exe") -Force
Ok "staged gst-plugin-scanner.exe into libexec\gstreamer-1.0"

# ---------------------------------------------------------------------------
# 4. Summary.
# ---------------------------------------------------------------------------
Step "Summary"
Info "Staging dir : $StagingDir"
Info ("Root files  : qcast-sender.exe + {0} runtime DLL(s)" -f $runtimeDlls.Count)
Info ("Plugins     : {0} of {1} curated + gstrswebrtc.dll" -f ($Plugins.Count - $missing.Count), $Plugins.Count)
Info "Scanner     : libexec\gstreamer-1.0\gst-plugin-scanner.exe"
if ($missing.Count -gt 0) {
  Warn ("MISSING plugin DLL(s) - check the GStreamer install completeness (ADDLOCAL=ALL): {0}" -f ($missing -join ", "))
  Warn "The installer will still build, but the missing element(s) will be unavailable at runtime."
}
Ok "Payload staged. Next: compile deploy\windows\qcast.iss with ISCC.exe (see README.md)."
