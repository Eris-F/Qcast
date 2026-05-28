#Requires -RunAsAdministrator
<#
  windows-setup.ps1 — make a fresh Windows box build-ready for Qcast, and optionally
  enable OpenSSH so it can be driven remotely (so the Tauri installer can be built +
  VALIDATED — the one step that needs real Windows). Works on a VM (see
  create-windows-vm.sh) or the dual-boot install.

  Run elevated, inside Windows:
      powershell -ExecutionPolicy Bypass -File windows-setup.ps1 -EnableSsh

  NOTE: authored on the Linux dev box and NOT yet run on Windows — review first.
  Mirrors the build prereqs in deploy\windows\README.md + release.yml's choco steps.
#>
[CmdletBinding()]
param([switch]$EnableSsh)
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# --- Chocolatey ---
if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
  Set-ExecutionPolicy Bypass -Scope Process -Force
  [System.Net.ServicePointManager]::SecurityProtocol = 3072
  Invoke-Expression ((New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1'))
  $env:Path += ";$env:ProgramData\chocolatey\bin"
}

# --- Build toolchain (VS C++ + Rust MSVC + GStreamer 1.26 MSVC runtime+devel) ---
choco install -y --no-progress git
choco install -y --no-progress visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
choco install -y --no-progress rust-ms              # Rust with the MSVC toolchain
choco install -y --no-progress gstreamer --version=1.26.0
choco install -y --no-progress gstreamer-devel --version=1.26.0
choco install -y --no-progress pkgconfiglite nodejs-lts

# --- cargo tooling for the build (cargo-c builds gstrswebrtc.dll; tauri-cli builds the app) ---
$env:Path += ";$env:USERPROFILE\.cargo\bin"
cargo install cargo-c
cargo install tauri-cli --version "^2"

# --- GStreamer env so gstreamer-rs/system-deps + gather-payload resolve the runtime ---
$root = "C:\gstreamer\1.0\msvc_x86_64"
[Environment]::SetEnvironmentVariable("GSTREAMER_1_0_ROOT_MSVC_X86_64", $root, "Machine")
[Environment]::SetEnvironmentVariable("PKG_CONFIG_PATH", "$root\lib\pkgconfig", "Machine")

# --- Optional: OpenSSH server, so the build can be driven from the Fedora host ---
if ($EnableSsh) {
  Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
  Set-Service -Name sshd -StartupType Automatic
  Start-Service sshd
  if (-not (Get-NetFirewallRule -Name 'sshd' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'sshd' -DisplayName 'OpenSSH Server (sshd)' `
      -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
  }
  Write-Host "`nOpenSSH enabled. Reach this box at one of:" -ForegroundColor Green
  Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -ne '127.0.0.1' } |
    Select-Object -ExpandProperty IPAddress
}

Write-Host "`nBuild-ready. Open a NEW shell (for PATH/env), then:" -ForegroundColor Green
Write-Host "  git clone <repo>; cd Qcast; cargo build -p qcast-sender   # compiles the SendInput injector"
Write-Host "  # scaffold the Tauri app per deploy\tauri\README.md, then:"
Write-Host "  deploy\tauri\build-windows.ps1                            # -> the NSIS -setup.exe"
