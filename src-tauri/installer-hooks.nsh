; NSIS install hooks for the Qcast Tauri bundle.
;
; Referenced by tauri.conf.json -> bundle.windows.nsis.installerHooks.
;
; WHY: Tauri stages bundle.resources under $INSTDIR\resources\. The GStreamer
; PLUGINS (resources\lib\gstreamer-1.0) and the scanner (resources\libexec\...) are
; found there by bundle.rs's resources\ candidate path. BUT the flat top-level
; GStreamer runtime DLLs (gstreamer-1.0-0.dll, glib-2.0-0.dll, gobject, gio, orc, the
; gst*-1.0 libs, ...) must sit on the EXE's DLL search path, and Windows does NOT
; search subfolders. So relocate those flat DLLs from resources\bin\ up next to the
; exe at install time. This is mitigation (a) for WINDOWS_INSTALLER.md risk #1.
;
; (Authored on Linux; validate on Windows — confirm the loader finds the runtime and
; gst-inspect-1.0 greens with no system GStreamer present.)

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Qcast: relocating bundled GStreamer runtime DLLs next to the executable..."
  CopyFiles /SILENT "$INSTDIR\resources\bin\*.dll" "$INSTDIR"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; The relocated DLLs were copies; remove them so uninstall leaves nothing behind.
  Delete "$INSTDIR\*.dll"
!macroend
