//! Relocatable-bundle support: makes a copied binary find GStreamer plugins that
//! ship *next to the exe*, so a packaged Qcast (Linux AppImage / Windows app dir)
//! runs without a system GStreamer install.
//!
//! [`configure_plugin_path`] MUST be called BEFORE `gst::init()` — GStreamer reads
//! `GST_PLUGIN_PATH` / `GST_PLUGIN_SCANNER` / `GST_PLUGIN_SYSTEM_PATH_1_0` only at
//! init time. For a normal dev build (no sibling bundled plugin dir) it is a strict
//! no-op: no env var is touched, so `cargo run` keeps using the system/user plugin
//! path exactly as before.

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

/// On non-Windows, the scanner has no extension; on Windows it's `.exe`.
#[cfg(windows)]
const SCANNER_NAME: &str = "gst-plugin-scanner.exe";
#[cfg(not(windows))]
const SCANNER_NAME: &str = "gst-plugin-scanner";

/// Discover GStreamer plugins bundled next to the executable and point GStreamer at
/// them. Call BEFORE `gst::init()`. No-op (touches no env var) when no bundled
/// plugin dir is found, so dev builds are unaffected.
pub fn configure_plugin_path() {
    let Ok(exe) = std::env::current_exe() else {
        tracing::debug!("bundle: current_exe() failed; leaving GStreamer paths untouched");
        return;
    };
    let Some(exe_dir) = exe.parent() else {
        tracing::debug!("bundle: exe has no parent dir; leaving GStreamer paths untouched");
        return;
    };

    // Candidate bundled plugin dirs, in priority order:
    //   <exedir>/../lib/gstreamer-1.0  — AppImage `usr/bin` → `usr/lib` layout
    //   <exedir>/lib/gstreamer-1.0     — Windows app-dir layout
    //   <exedir>/gstreamer-1.0         — flat layout
    // First existing wins for ordering, but we prepend every one that exists.
    let candidates = [
        exe_dir.join("..").join("lib").join("gstreamer-1.0"),
        exe_dir.join("lib").join("gstreamer-1.0"),
        exe_dir.join("gstreamer-1.0"),
    ];

    let found: Vec<PathBuf> = candidates.into_iter().filter(|p| p.is_dir()).collect();
    if found.is_empty() {
        // Normal dev build: no bundled dir → strict no-op.
        tracing::debug!("bundle: no bundled GStreamer plugin dir next to exe; using system paths");
        return;
    }

    // Prepend each found dir to GST_PLUGIN_PATH, preserving any existing value.
    let sep = if cfg!(windows) { ';' } else { ':' };
    let existing = std::env::var_os("GST_PLUGIN_PATH");
    let mut parts: Vec<String> = found
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if let Some(prev) = existing.as_ref() {
        if !prev.is_empty() {
            parts.push(prev.to_string_lossy().into_owned());
        }
    }
    let new_path = parts.join(&sep.to_string());
    std::env::set_var("GST_PLUGIN_PATH", &new_path);
    tracing::debug!(path = %new_path, "bundle: prepended bundled plugin dir(s) to GST_PLUGIN_PATH");

    // Bundled plugin scanner, if shipped. AppImage: <exedir>/../libexec/...;
    // Windows/flat: <exedir>/libexec/...
    let scanner_candidates = [
        exe_dir
            .join("..")
            .join("libexec")
            .join("gstreamer-1.0")
            .join(SCANNER_NAME),
        exe_dir
            .join("libexec")
            .join("gstreamer-1.0")
            .join(SCANNER_NAME),
    ];
    if let Some(scanner) = scanner_candidates.iter().find(|p| p.is_file()) {
        std::env::set_var("GST_PLUGIN_SCANNER", scanner);
        tracing::debug!(scanner = %scanner.display(), "bundle: set GST_PLUGIN_SCANNER");
    }

    // Only an explicitly-packaged run (QCAST_BUNDLE=1) AND a real bundled dir should
    // disable the system plugin path — this stops the AppImage from picking up
    // mismatched host plugins. A plain copied dev binary leaves system paths alone.
    if std::env::var("QCAST_BUNDLE").as_deref() == Ok("1") {
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", "");
        tracing::debug!("bundle: QCAST_BUNDLE=1 — cleared GST_PLUGIN_SYSTEM_PATH_1_0");
    }
}

/// Test-only helper exposing the candidate-dir logic so we can assert ordering /
/// no-op behaviour without mutating process env. Returns the bundled plugin dirs
/// that exist under `exe_dir`, in priority order.
#[cfg(test)]
pub fn bundled_plugin_dirs(exe_dir: &Path) -> Vec<PathBuf> {
    [
        exe_dir.join("..").join("lib").join("gstreamer-1.0"),
        exe_dir.join("lib").join("gstreamer-1.0"),
        exe_dir.join("gstreamer-1.0"),
    ]
    .into_iter()
    .filter(|p| p.is_dir())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bundled_dirs_means_empty() {
        let tmp = std::env::temp_dir().join(format!("qcast-bundle-test-empty-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(bundled_plugin_dirs(&tmp).is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detects_appimage_and_flat_layouts_in_order() {
        let base = std::env::temp_dir().join(format!("qcast-bundle-test-{}", std::process::id()));
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // AppImage layout: <base>/lib/gstreamer-1.0 (parent of bin)
        let appimage = base.join("lib").join("gstreamer-1.0");
        std::fs::create_dir_all(&appimage).unwrap();
        // Flat layout: <bin>/gstreamer-1.0
        let flat = bin.join("gstreamer-1.0");
        std::fs::create_dir_all(&flat).unwrap();

        let dirs = bundled_plugin_dirs(&bin);
        assert_eq!(dirs.len(), 2, "expected appimage + flat dirs: {dirs:?}");
        // Priority: ../lib first, then flat. Compare canonicalized forms.
        assert_eq!(dirs[0].canonicalize().unwrap(), appimage.canonicalize().unwrap());
        assert_eq!(dirs[1].canonicalize().unwrap(), flat.canonicalize().unwrap());

        let _ = std::fs::remove_dir_all(&base);
    }
}
