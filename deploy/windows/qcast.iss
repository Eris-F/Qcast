; qcast.iss - Inno Setup 6 script for the Qcast Windows bundled installer.
;
; Produces Qcast-Setup-<version>.exe: a per-user installer that ships the prebuilt
; qcast-sender.exe + the GStreamer MSVC runtime DLLs + the curated plugin set
; (including our gstrswebrtc.dll) + the gst-plugin-scanner. The end user installs
; and runs Qcast with NO winget / Rust / MSVC / compile step.
;
; The payload is assembled FIRST by deploy\windows\gather-payload.ps1 into a staging
; dir; this script just packs that dir 1:1 into {app}. The staging layout matches
; crates\qcast-sender\src\bundle.rs's Windows expectations:
;
;     {app}\qcast-sender.exe                               <- the GUI host
;     {app}\*.dll                                          <- GStreamer runtime DLLs
;     {app}\lib\gstreamer-1.0\*.dll                        <- plugins (incl. gstrswebrtc.dll)
;     {app}\libexec\gstreamer-1.0\gst-plugin-scanner.exe   <- the scanner
;
; bundle.rs, BEFORE gst::init(), resolves "<exedir>\lib\gstreamer-1.0" and
; "<exedir>\libexec\gstreamer-1.0\gst-plugin-scanner.exe", prepends the plugin dir
; to GST_PLUGIN_PATH and sets GST_PLUGIN_SCANNER. So the bundled plugins are found
; with NO env var set by this installer.
;
; Build:  ISCC.exe deploy\windows\qcast.iss
;         ISCC.exe /DStagingDir="C:\path\to\staging" /DAppVersion=0.1.0 deploy\windows\qcast.iss
; See deploy\windows\README.md for the full build + signing sequence.

; ---- Configurable defines (override on the ISCC.exe command line with /D...) ----
#ifndef AppVersion
  #define AppVersion "0.1.0"      ; keep in sync with crates\qcast-sender\Cargo.toml
#endif
#ifndef StagingDir
  ; Default: the staging dir produced by gather-payload.ps1 next to this script.
  #define StagingDir "staging"
#endif

#define AppName "Qcast"
#define AppPublisher "Qcast"
#define AppExeName "qcast-sender.exe"

[Setup]
AppId={{8D3B6F2E-1C4A-4E6B-9F1D-Q3CAST000001}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
VersionInfoVersion={#AppVersion}

; Per-user install: no admin / no UAC prompt for a smoother first run. Installs to
; %LOCALAPPDATA%\Programs\Qcast. (Revisit if a system-wide install is ever wanted -
; that would need PrivilegesRequired=admin and a {commonpf}\Qcast DefaultDirName.)
PrivilegesRequired=lowest
DefaultDirName={localappdata}\Programs\Qcast
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes

; 64-bit only (GStreamer MSVC x86_64 runtime + d3d11 capture). x64compatible also
; allows ARM64 machines running x64 binaries under emulation.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Output: Qcast-Setup-<version>.exe in the Output\ dir next to this script.
OutputDir=Output
OutputBaseFilename=Qcast-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#AppExeName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; Pack the ENTIRE staging dir 1:1 into {app}. gather-payload.ps1 already laid out
; qcast-sender.exe + runtime *.dll at the root, plugins under lib\gstreamer-1.0,
; and the scanner under libexec\gstreamer-1.0 - so recursesubdirs preserves the
; exact tree bundle.rs expects. (recursesubdirs walks subfolders; createallsubdirs
; recreates even empty ones.)
Source: "{#StagingDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion

[Icons]
; Start-Menu shortcut to the GUI host (default run takes no args -> shows the GUI
; with the viewer password). NOTE on env vars: a Start-Menu .lnk cannot set process
; env vars, so we cannot inject QCAST_BUNDLE=1 here. We rely on bundle.rs auto-
; detecting the sibling lib\gstreamer-1.0 and prepending it to GST_PLUGIN_PATH,
; which is sufficient when the host has NO conflicting GStreamer. If a user DOES
; have a different GStreamer version installed system-wide and hits plugin
; conflicts, QCAST_BUNDLE=1 is needed so bundle.rs clears GST_PLUGIN_SYSTEM_PATH_1_0.
; RECOMMENDED robust fix (to implement when this is tested on real Windows, NOT in
; this authoring phase): make bundle.rs treat "a sibling lib\gstreamer-1.0 exists"
; as implying bundle mode on Windows (clear the system path even without
; QCAST_BUNDLE=1) - a one-line app-side change is cleaner than a wrapper launcher
; or a registry/env-var hack. Alternative stop-gap: ship a tiny qcast.cmd wrapper
; ("set QCAST_BUNDLE=1 & start qcast-sender.exe") and point the shortcut at it.
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
; Offer to launch the GUI right after install (no args -> shows the password UI).
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
