#!/usr/bin/env bash
#
# build-appimage.sh — produce a self-contained Qcast-x86_64.AppImage.
#
# Bundles: the release qcast-sender binary + the GStreamer runtime libraries +
# the plugin set it needs (coreelements, videoconvertscale, vpx, webrtc/webrtcbin,
# rtp/rtpmanager, nice, dtls, srtp, sctp, pipewire, ...) + OUR webrtcsink plugin
# (libgstrswebrtc.so) + rtpgccbwe (libgstrsrtp.so, congestion control) + the
# gst-plugin-scanner. The result runs on a typical Linux desktop with no system
# GStreamer install.
#
# Reproducible & re-runnable: downloads tooling into a gitignored cache dir and
# stages everything under a gitignored build dir. Re-run after code changes.
#
# Usage:   deploy/appimage/build-appimage.sh
# Env knobs (all optional, sensible defaults below):
#   QCAST_PLUGIN_SO   path to our libgstrswebrtc.so
#   GST_SYS_PLUGINS   system GStreamer plugins dir   (Fedora: /usr/lib64/gstreamer-1.0)
#   GST_SYS_HELPERS   system GStreamer helpers dir   (has gst-plugin-scanner)
#   SKIP_CARGO_BUILD  set to 1 to skip the cargo build step
set -euo pipefail

# ---------------------------------------------------------------------------
# Paths (parameterized)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CACHE_DIR="$SCRIPT_DIR/cache"          # gitignored: downloaded tooling
BUILD_DIR="$SCRIPT_DIR/build"          # gitignored: AppDir + output
APPDIR="$BUILD_DIR/AppDir"
STAGE_PLUGINS="$BUILD_DIR/stage-plugins" # combined system + our plugin dir

DESKTOP_FILE="$SCRIPT_DIR/qcast.desktop"
ICON_FILE="$SCRIPT_DIR/qcast.png"

BIN_SRC="$REPO_ROOT/target/release/qcast-sender"
QCAST_PLUGIN_SO="${QCAST_PLUGIN_SO:-$HOME/.local/share/gstreamer-1.0/plugins/libgstrswebrtc.so}"
# rtpgccbwe (Google Congestion Control) lives in the gst-plugins-rs RTP plugin.
# webrtcsink uses it for adaptive bitrate / congestion control — reliability is
# this project's #1 priority. It is a Rust plugin installed alongside ours in
# ~/.local, NOT in the system plugin dir, so it is staged specially (below).
QCAST_RSRTP_SO="${QCAST_RSRTP_SO:-$HOME/.local/share/gstreamer-1.0/plugins/libgstrsrtp.so}"
GST_SYS_PLUGINS="${GST_SYS_PLUGINS:-/usr/lib64/gstreamer-1.0}"
GST_SYS_HELPERS="${GST_SYS_HELPERS:-/usr/libexec/gstreamer-1.0}"

LINUXDEPLOY="$CACHE_DIR/linuxdeploy-x86_64.AppImage"
LDP_GST="$CACHE_DIR/linuxdeploy-plugin-gstreamer.sh"
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
LDP_GST_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gstreamer/master/linuxdeploy-plugin-gstreamer.sh"

OUTPUT="$BUILD_DIR/Qcast-x86_64.AppImage"

log() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 1. Build the release binary (the long pole). Skippable.
# ---------------------------------------------------------------------------
if [ "${SKIP_CARGO_BUILD:-0}" != "1" ]; then
  log "cargo build --release -p qcast-sender"
  # RUST_MIN_STACK + limited codegen jobs work around a Fedora rustc/LLVM
  # SIGSEGV seen under heavy parallel opt-level=3 codegen. cargo resumes on
  # re-run, so a transient crash is harmless.
  ( cd "$REPO_ROOT" && RUST_MIN_STACK=33554432 CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
      cargo build --release -p qcast-sender )
fi
[ -x "$BIN_SRC" ] || die "release binary not found: $BIN_SRC (run cargo build first)"
[ -f "$QCAST_PLUGIN_SO" ] || die "our webrtcsink plugin not found: $QCAST_PLUGIN_SO"
[ -d "$GST_SYS_PLUGINS" ] || die "system GStreamer plugins dir not found: $GST_SYS_PLUGINS"
[ -d "$GST_SYS_HELPERS" ] || die "system GStreamer helpers dir not found: $GST_SYS_HELPERS"

# ---------------------------------------------------------------------------
# 2. Fetch tooling into the gitignored cache dir.
# ---------------------------------------------------------------------------
mkdir -p "$CACHE_DIR"
if [ ! -f "$LINUXDEPLOY" ]; then
  log "Downloading linuxdeploy"
  curl -fSL -o "$LINUXDEPLOY" "$LINUXDEPLOY_URL"
fi
if [ ! -f "$LDP_GST" ]; then
  log "Downloading linuxdeploy-plugin-gstreamer"
  curl -fSL -o "$LDP_GST" "$LDP_GST_URL"
fi
chmod +x "$LINUXDEPLOY" "$LDP_GST"

# Extract linuxdeploy into the cache dir. This (a) avoids relying on FUSE and
# (b) exposes the `patchelf` it bundles, which the gstreamer plugin needs to set
# rpaths but which is NOT installed system-wide on this host. Re-extract only if
# missing (reproducible + fast on re-run).
LD_EXTRACT="$CACHE_DIR/linuxdeploy-extracted"
LD_BIN="$LD_EXTRACT/squashfs-root/usr/bin/linuxdeploy"
if [ ! -x "$LD_BIN" ]; then
  log "Extracting linuxdeploy (for patchelf + FUSE-free run)"
  rm -rf "$LD_EXTRACT"
  mkdir -p "$LD_EXTRACT"
  ( cd "$LD_EXTRACT" && "$LINUXDEPLOY" --appimage-extract >/dev/null )
fi
[ -x "$LD_BIN" ] || die "could not extract linuxdeploy binary"
LD_TOOLS_BIN="$LD_EXTRACT/squashfs-root/usr/bin"
[ -x "$LD_TOOLS_BIN/patchelf" ] || die "bundled patchelf not found in linuxdeploy"
# Run the extracted binary directly (no FUSE needed).
LD_RUN=("$LD_BIN")

# ---------------------------------------------------------------------------
# 3. Fresh AppDir + staged plugin dir.
# ---------------------------------------------------------------------------
log "Laying out AppDir at $APPDIR"
rm -rf "$BUILD_DIR"
mkdir -p "$APPDIR/usr/bin" \
         "$APPDIR/usr/lib/gstreamer-1.0" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
         "$STAGE_PLUGINS"

install -m 0755 "$BIN_SRC"      "$APPDIR/usr/bin/qcast-sender"
install -m 0644 "$DESKTOP_FILE" "$APPDIR/usr/share/applications/qcast.desktop"
install -m 0644 "$ICON_FILE"    "$APPDIR/usr/share/icons/hicolor/256x256/apps/qcast.png"
# Our plugins also placed directly in the AppDir's plugin dir so they survive
# even if the gstreamer-plugin pass were skipped.
install -m 0755 "$QCAST_PLUGIN_SO" "$APPDIR/usr/lib/gstreamer-1.0/libgstrswebrtc.so"
if [ -f "$QCAST_RSRTP_SO" ]; then
  install -m 0755 "$QCAST_RSRTP_SO" "$APPDIR/usr/lib/gstreamer-1.0/libgstrsrtp.so"
else
  echo "  warning: rtpgccbwe plugin not found, skipping: $QCAST_RSRTP_SO"
fi

# CURATED plugin set. The gstreamer plugin copies EVERY file from this dir into
# the AppDir, then linuxdeploy resolves the transitive shared-lib closure of all
# of them. Copying the *whole* 240-plugin system dir drags in GTK, Qt5/6, samba,
# ffmpeg, etc. — a giant lib closure whose bundled glib/gobject/gio then mismatch
# the host stack and SIGSEGV in a library constructor at load time. So we stage
# only the plugins Qcast's pipeline actually uses (mapped via gst-inspect):
#   coreelements        queue/capsfilter/tee/identity/...
#   videoconvertscale   videoconvert, videoscale
#   vpx                 vp8enc/vp8dec (royalty-free, preferred codec)
#   webrtc              webrtcbin — webrtcsink instantiates this internally (CRITICAL)
#   rtp + rtpmanager    rtp payloaders, rtpbin
#   nice/dtls/srtp/sctp webrtc transport (ICE / DTLS-SRTP / data channel)
#   rswebrtc (ours)     webrtcsink
#   rsrtp (staged sep.) rtpgccbwe — Google congestion control (adaptive bitrate)
#   app                 appsrc/appsink (webrtcsink internals)
#   typefindfunctions   caps negotiation
#   videotestsrc        --source test
#   pipewiresrc         real Wayland/portal screen capture (host portal at runtime)
#   ximagesrc           X11 fallback capture
#   audiotestsrc/opus   optional audio track webrtcsink may negotiate
#   autodetect/playback safe helpers webrtcsink may use internally
QCAST_PLUGINS=(
  libgstcoreelements.so
  libgstvideoconvertscale.so
  libgstvideorate.so          # webrtcsink codec-discovery pipeline needs videorate
  libgstvpx.so
  libgstencoding.so           # encodebin — webrtcsink encode path
  libgstdebugutilsbad.so      # errorignore — used inside webrtcsink discovery
  libgstwebrtc.so             # webrtcbin — webrtcsink creates this internally (CRITICAL)
  libgstrtp.so
  libgstrtpmanager.so
  libgstnice.so
  libgstdtls.so
  libgstsrtp.so
  libgstsctp.so
  libgstapp.so
  libgsttypefindfunctions.so
  libgstvideotestsrc.so
  libgstpipewire.so
  libgstximagesrc.so
  libgstaudiotestsrc.so
  libgstaudioconvert.so       # audio track resampling/convert (if negotiated)
  libgstaudioresample.so
  libgstopus.so
  libgstautodetect.so
  libgstplayback.so
)
log "Staging curated plugin set (${#QCAST_PLUGINS[@]} system plugins + libgstrswebrtc.so)"
for so in "${QCAST_PLUGINS[@]}"; do
  if [ -f "$GST_SYS_PLUGINS/$so" ]; then
    cp -a "$GST_SYS_PLUGINS/$so" "$STAGE_PLUGINS/"
  else
    echo "  warning: system plugin not found, skipping: $so"
  fi
done
cp -a "$QCAST_PLUGIN_SO" "$STAGE_PLUGINS/libgstrswebrtc.so"
# rtpgccbwe (gst-plugins-rs RTP) — staged from ~/.local, not the system dir.
if [ -f "$QCAST_RSRTP_SO" ]; then
  cp -a "$QCAST_RSRTP_SO" "$STAGE_PLUGINS/libgstrsrtp.so"
  log "Staged rtpgccbwe plugin (libgstrsrtp.so) for congestion control"
else
  echo "  warning: rtpgccbwe plugin not found, skipping: $QCAST_RSRTP_SO"
fi

# ---------------------------------------------------------------------------
# 4. Custom AppRun hook: force the binary onto the BUNDLED plugins.
#    QCAST_BUNDLE=1 makes bundle.rs clear GST_PLUGIN_SYSTEM_PATH_1_0 so a host
#    GStreamer (possibly a mismatched version) is never consulted.
# ---------------------------------------------------------------------------
mkdir -p "$APPDIR/apprun-hooks"
cat > "$APPDIR/apprun-hooks/qcast-env.sh" <<'EOF'
#! /bin/bash
# Force Qcast to use the plugins bundled in this AppImage (see bundle.rs).
export QCAST_BUNDLE=1
EOF
chmod +x "$APPDIR/apprun-hooks/qcast-env.sh"

# ---------------------------------------------------------------------------
# 5. Run linuxdeploy with the gstreamer plugin.
# ---------------------------------------------------------------------------
log "Running linuxdeploy + gstreamer plugin"
# linuxdeploy discovers `--plugin gstreamer` by finding an executable named
# `linuxdeploy-plugin-gstreamer.sh` on PATH, so expose the cache dir.
# CACHE_DIR holds the gstreamer plugin script (for --plugin discovery);
# LD_TOOLS_BIN holds the bundled patchelf the plugin shells out to.
export PATH="$CACHE_DIR:$LD_TOOLS_BIN:$PATH"
export LINUXDEPLOY="$LD_BIN"              # plugin invokes it internally (FUSE-free)
# The strip shipped inside linuxdeploy's AppImage is older binutils and chokes
# on the modern `.relr.dyn` ELF section produced by Fedora's toolchain, which
# linuxdeploy treats as fatal. NO_STRIP=1 skips stripping (honored by both the
# outer and the gstreamer-plugin's inner `linuxdeploy --appdir` call).
export NO_STRIP=1
export GSTREAMER_VERSION="1.0"
export GSTREAMER_INCLUDE_BAD_PLUGINS="1"   # we need -bad (webrtc deps, sctp, ...)
export GSTREAMER_PLUGINS_DIR="$STAGE_PLUGINS"   # curated set (Fedora lib64 subset + ours)
export GSTREAMER_HELPERS_DIR="$GST_SYS_HELPERS"  # gst-plugin-scanner lives here on Fedora
# Make our plugin discoverable to any GST_PLUGIN_PATH-based discovery too.
export GST_PLUGIN_PATH="$STAGE_PLUGINS"

# Libraries that MUST come from the host, never the bundle. linuxdeploy puts the
# AppDir's usr/lib FIRST on the loader path, so a bundled copy shadows the host's
# — and a bundled glib/gobject/gio/GL/X11/wayland built against a different ABI
# crashes in a library constructor at load time (SIGSEGV in call_init). Every
# modern Linux desktop already ships these, so excluding them is both safe and
# the documented fix. (linuxdeploy's built-in excludelist covers glibc/X core but
# NOT the glib stack or GL/driver libs.)
EXCLUDES=(
  'libglib-2.0.so*' 'libgobject-2.0.so*' 'libgio-2.0.so*' 'libgmodule-2.0.so*'
  'libgthread-2.0.so*' 'libffi.so*' 'libpcre2-8.so*'
  'libGL.so*' 'libGLX.so*' 'libGLdispatch.so*' 'libEGL.so*' 'libGLESv2.so*'
  'libgbm.so*' 'libdrm.so*' 'libvulkan.so*'
  'libwayland-client.so*' 'libwayland-server.so*' 'libwayland-cursor.so*'
  'libwayland-egl.so*' 'libxkbcommon.so*'
  'libX11.so*' 'libX11-xcb.so*' 'libxcb*.so*' 'libXext.so*' 'libXrandr.so*'
  'libXrender.so*' 'libXi.so*' 'libXfixes.so*' 'libXcursor.so*' 'libXdamage.so*'
  'libXcomposite.so*' 'libXinerama.so*' 'libXtst.so*' 'libXau.so*' 'libXv.so*'
  'libdbus-1.so*' 'libsystemd.so*' 'libudev.so*' 'libselinux.so*'
  'libpipewire-0.3.so*' 'libpulse.so*' 'libasound.so*'
  'libstdc++.so*' 'libgcc_s.so*' 'libm.so*'
)
EXCLUDE_ARGS=()
for pat in "${EXCLUDES[@]}"; do EXCLUDE_ARGS+=( --exclude-library "$pat" ); done

# STEP A — populate the AppDir (deploy libs + run gstreamer plugin). NO --output
# yet: we must repair patchelf damage before packing (see STEP B).
"${LD_RUN[@]}" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/qcast-sender" \
  --desktop-file "$DESKTOP_FILE" \
  --icon-file "$ICON_FILE" \
  "${EXCLUDE_ARGS[@]}" \
  --plugin gstreamer

# ---------------------------------------------------------------------------
# STEP B — repair patchelf corruption.
#
# linuxdeploy AND the gstreamer plugin both run `patchelf --set-rpath` on every
# deployed ELF — the executable, the shared libraries, the plugins AND the
# gst-plugin-scanner. The patchelf bundled inside linuxdeploy (0.15.0) CORRUPTS
# ELF files that carry DT_RELR relative relocations (everything built by Fedora's
# modern toolchain): it shifts sections without fixing DT_RELR, so DT_INIT no
# longer resolves and the process SIGSEGVs in a constructor at load time (jump to
# a zeroed page). The patchelf'd executable likewise crashes immediately.
#
# Fix: overwrite every deployed ELF with a PRISTINE, un-patchelf'd copy from the
# system / our staged dir / our build tree. We don't need the rpath patchelf was
# trying to set — the AppRun hooks put `usr/lib` on LD_LIBRARY_PATH and the
# plugins/scanner are found via GST_PLUGIN_PATH / GST_PLUGIN_SCANNER, so ELFs
# resolve their deps without any embedded rpath.
log "Repairing patchelf-corrupted ELF objects with pristine copies"
restore_from_system() {
  # $1 = path to deployed .so under the AppDir
  local f="$1" base sys d
  base="$(basename "$f")"
  for d in /usr/lib64 /lib64 /usr/lib "$(dirname "$(readlink -f /lib64)")"; do
    if [ -e "$d/$base" ]; then
      sys="$(readlink -f "$d/$base")"
      [ -f "$sys" ] && { command cp -f --remove-destination "$sys" "$f"; return 0; }
    fi
  done
  return 1
}
repaired=0; unresolved=0
for f in "$APPDIR"/usr/lib/*.so*; do
  [ -f "$f" ] || continue
  if restore_from_system "$f"; then repaired=$((repaired+1));
  else unresolved=$((unresolved+1)); echo "  note: no system copy for $(basename "$f") (leaving as-is)"; fi
done
# Plugins: restore from our pristine staged dir (libgstrswebrtc.so included).
for f in "$APPDIR"/usr/lib/gstreamer-1.0/*.so; do
  [ -f "$f" ] || continue
  base="$(basename "$f")"
  if [ -f "$STAGE_PLUGINS/$base" ]; then
    command cp -f --remove-destination "$STAGE_PLUGINS/$base" "$f"; repaired=$((repaired+1))
  fi
done
# The executable itself: linuxdeploy patchelf'd it and bloated/broke it. Restore
# the pristine release binary.
command cp -f --remove-destination "$BIN_SRC" "$APPDIR/usr/bin/qcast-sender"
chmod 0755 "$APPDIR/usr/bin/qcast-sender"; repaired=$((repaired+1))
# The gst-plugin-scanner: also patchelf-corrupted. Without a working scanner,
# external plugins (vpx, etc.) silently fail to register ("External plugin loader
# failed") and webrtcsink can't find an encoder. Restore it pristine.
SCANNER_DST="$APPDIR/usr/lib/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner"
if [ -f "$SCANNER_DST" ] && [ -f "$GST_SYS_HELPERS/gst-plugin-scanner" ]; then
  command cp -f --remove-destination "$GST_SYS_HELPERS/gst-plugin-scanner" "$SCANNER_DST"
  chmod 0755 "$SCANNER_DST"; repaired=$((repaired+1))
fi
log "Repaired $repaired ELF objects ($unresolved libs had no system source)"

# ---------------------------------------------------------------------------
# STEP C — pack the AppImage from the repaired AppDir with appimagetool.
#
# We must NOT re-run linuxdeploy here: every linuxdeploy invocation that touches
# the AppDir re-runs the corrupting patchelf over our freshly-repaired libraries.
# appimagetool only builds the squashfs image (no patchelf), so the repaired
# libraries are preserved. appimagetool ships inside the linuxdeploy AppImage we
# already extracted.
APPIMAGETOOL="$LD_EXTRACT/squashfs-root/plugins/linuxdeploy-plugin-appimage/usr/bin/appimagetool"
[ -x "$APPIMAGETOOL" ] || die "bundled appimagetool not found: $APPIMAGETOOL"
# appimagetool also calls mksquashfs; it ships its own. Run extract-and-run-free
# by invoking the extracted binary directly. ARCH must be set for the runtime.
log "Packing AppImage with appimagetool"
rm -f "$OUTPUT"
ARCH=x86_64 NO_STRIP=1 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUTPUT"

[ -f "$OUTPUT" ] || die "AppImage was not produced"
chmod +x "$OUTPUT"
log "Built: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
