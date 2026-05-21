#!/usr/bin/env bash
#
# Qcast — Linux guided setup.
#
# Installs every runtime + build dependency Qcast needs, builds the gst-plugins-rs
# `webrtcsink` plugin (not packaged on most distros) into the user plugin dir, and
# builds the Qcast host binary. Designed to be run ONCE on a fresh machine and to
# be safe to re-run (idempotent): each step checks whether it's already satisfied
# before doing work.
#
# Intentionally verbose and defensive — the whole point is a clean first install
# with no surprise "missing element" / "no encoder" bugs at runtime. Big is fine.
#
# Supported package managers: dnf (Fedora/RHEL), apt (Debian/Ubuntu),
# pacman (Arch), zypper (openSUSE). Other distros: install the equivalents of the
# package groups below by hand, then re-run to let the verification step confirm.
#
# Usage:
#   bash deploy/setup-linux.sh            # full setup
#   bash deploy/setup-linux.sh --verify   # only re-run the verification checks
#   bash deploy/setup-linux.sh --no-build # install deps + webrtcsink, skip cargo build
#
# Does NOT require passwordless sudo; it will call `sudo` for system package
# installs and prompt you normally. Everything else stays in your home dir.

set -euo pipefail

# ----------------------------------------------------------------------------
# Pretty logging
# ----------------------------------------------------------------------------
if [[ -t 1 ]]; then
  BOLD=$'\e[1m'; RED=$'\e[31m'; GRN=$'\e[32m'; YLW=$'\e[33m'; BLU=$'\e[34m'; RST=$'\e[0m'
else
  BOLD=''; RED=''; GRN=''; YLW=''; BLU=''; RST=''
fi
step()  { printf '\n%s==>%s %s%s\n' "$BLU$BOLD" "$RST$BOLD" "$*" "$RST"; }
info()  { printf '    %s\n' "$*"; }
ok()    { printf '    %s✔%s %s\n' "$GRN" "$RST" "$*"; }
warn()  { printf '    %s!%s %s\n' "$YLW" "$RST" "$*" >&2; }
die()   { printf '\n%sERROR:%s %s\n' "$RED$BOLD" "$RST" "$*" >&2; exit 1; }

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGIN_DIR="${HOME}/.local/share/gstreamer-1.0/plugins"
GST_PLUGINS_RS_REF="0.15"   # branch matching GStreamer 1.26 / gstreamer-rs 0.25
BUILD_DIR="${HOME}/.cache/qcast-build"

ONLY_VERIFY=0
DO_BUILD=1
for arg in "$@"; do
  case "$arg" in
    --verify)   ONLY_VERIFY=1 ;;
    --no-build) DO_BUILD=0 ;;
    -h|--help)  grep -E '^#( |$)' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $arg" ;;
  esac
done

# ----------------------------------------------------------------------------
# retry: run a command up to 3 times (network installs flake)
# ----------------------------------------------------------------------------
retry() {
  local n=0 max=3
  until "$@"; do
    n=$((n+1))
    (( n >= max )) && return 1
    warn "command failed (attempt $n/$max), retrying in 3s: $*"
    sleep 3
  done
}

# ----------------------------------------------------------------------------
# Distro / package-manager detection
# ----------------------------------------------------------------------------
PKG=""           # dnf | apt | pacman | zypper
detect_distro() {
  step "Detecting distribution"
  [[ -r /etc/os-release ]] || die "/etc/os-release not found; unsupported system"
  # shellcheck disable=SC1091
  . /etc/os-release
  local id="${ID:-} ${ID_LIKE:-}"
  if   command -v dnf    >/dev/null && [[ "$id" == *fedora*  || "$id" == *rhel* || "$id" == *centos* ]]; then PKG=dnf
  elif command -v apt-get>/dev/null && [[ "$id" == *debian*  || "$id" == *ubuntu* ]]; then PKG=apt
  elif command -v pacman >/dev/null && [[ "$id" == *arch*    ]]; then PKG=pacman
  elif command -v zypper >/dev/null && [[ "$id" == *suse*    ]]; then PKG=zypper
  elif command -v dnf    >/dev/null; then PKG=dnf
  elif command -v apt-get>/dev/null; then PKG=apt
  elif command -v pacman >/dev/null; then PKG=pacman
  elif command -v zypper >/dev/null; then PKG=zypper
  else die "no supported package manager (dnf/apt/pacman/zypper) found"
  fi
  ok "${PRETTY_NAME:-$ID} — using ${PKG}"
}

SUDO=""
# Pick `sudo` only when we're not already root. Written as an if (not
# `[[ ... ]] && SUDO=sudo`) on purpose: under `set -e`, the && form returns the
# failed test's nonzero status when running AS root, which would abort the whole
# script before any work is done. The if always returns 0.
need_sudo() {
  if [[ $EUID -ne 0 ]]; then
    SUDO="sudo"
    command -v sudo >/dev/null 2>&1 \
      || die "not running as root and 'sudo' is not installed — re-run as root, or install sudo first"
  else
    SUDO=""
  fi
}

pkg_install() {
  # Install a list of packages; missing-package errors are tolerated per-distro
  # because names differ across versions (we verify elements at the end anyway).
  # IMPORTANT: the final verify() is the strict guard — a tolerated install
  # failure here only matters if it leaves a *required* element missing, which
  # verify will then catch and explain. We never let a flaky/partial install
  # abort the whole run, but we also never claim success it didn't earn.
  local pkgs=("$@")
  # $SUDO is intentionally unquoted so it word-splits to nothing when running as
  # root (need_sudo leaves it empty) and to `sudo` otherwise.
  # shellcheck disable=SC2086
  case "$PKG" in
    # dnf5 (Fedora 41+) and dnf4 both accept --skip-unavailable on `install`,
    # so unknown/renamed names are dropped instead of failing the batch. No
    # second "plain install" fallback: re-running without --skip-unavailable
    # would just hard-fail on the first missing name and be swallowed by
    # `|| true`, hiding nothing useful. One tolerant pass is enough.
    dnf)    retry $SUDO dnf install -y --skip-unavailable --skip-broken "${pkgs[@]}" || true ;;
    apt)    retry $SUDO apt-get update -y || true
            retry $SUDO apt-get install -y --no-install-recommends "${pkgs[@]}" || true ;;
    # NOTE: `-Sy` (refresh without a full `-Syu`) risks an Arch partial-upgrade
    # if the mirror has moved on; on a fresh machine that's rare. If a dependency
    # version mismatch appears, run `sudo pacman -Syu` once, then re-run this.
    pacman) retry $SUDO pacman -Sy --needed --noconfirm "${pkgs[@]}" || true ;;
    # `--ignore-unknown` is the zypper analogue of dnf's --skip-unavailable: a
    # name that doesn't exist (renamed/optional) is dropped instead of aborting
    # the whole batch. Without it one bad name leaves NOTHING installed.
    zypper) retry $SUDO zypper --non-interactive --ignore-unknown install --no-recommends "${pkgs[@]}" || true ;;
  esac
}

# ----------------------------------------------------------------------------
# 1. System packages: GStreamer stack, capture, TURN relay, build tools
# ----------------------------------------------------------------------------
install_system_packages() {
  step "Installing GStreamer + capture + TURN + build dependencies"
  case "$PKG" in
    dnf)
      # NOTE on package names (Fedora 43, dnf5):
      #  - libav plugin is `gstreamer1-plugin-libav` (there is NO `gstreamer1-libav`).
      #  - VAAPI elements (vah264enc, vah264lpenc) come from `gstreamer1-plugins-bad-free`.
      #  - `intel-media-driver` (the iHD VAAPI driver for the Intel laptop target)
      #    lives in RPM Fusion *nonfree*, which is not enabled on a stock Fedora.
      #    If absent it is simply skipped; software encode still works, and the
      #    informational "Hardware H.264 encoder" check will note it.
      pkg_install \
        gstreamer1 gstreamer1-plugins-base gstreamer1-plugins-good \
        gstreamer1-plugins-bad-free gstreamer1-plugins-bad-free-extras \
        gstreamer1-plugins-ugly-free gstreamer1-plugin-libav \
        libnice libnice-gstreamer1 \
        pipewire pipewire-gstreamer \
        libva libva-utils mesa-va-drivers intel-media-driver \
        gstreamer1-devel gstreamer1-plugins-base-devel \
        cargo-c git curl gcc gcc-c++ make pkgconf-pkg-config openssl-devel \
        xdg-desktop-portal
      # openh264 (software H.264) lives in the fedora-cisco-openh264 repo, which
      # ships enabled on Fedora but may be off on RHEL/CentOS. Optional.
      pkg_install gstreamer1-plugin-openh264 || warn "openh264 plugin optional"
      ;;
    apt)
      # NOTE: `gstreamer1.0-nice` (the ICE plugin providing nicesink) is in the
      # *universe* component on Ubuntu; if it can't be found, enable universe
      # (`sudo add-apt-repository universe`) and re-run. `cargo-c` is packaged on
      # recent Debian/Ubuntu — installing it here is far faster than the
      # `cargo install cargo-c` fallback in ensure_cargo_c().
      pkg_install \
        gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
        gstreamer1.0-nice gstreamer1.0-pipewire libnice10 \
        pipewire libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
        libssl-dev pkg-config build-essential git curl cargo-c \
        va-driver-all intel-media-va-driver xdg-desktop-portal
      ;;
    pacman)
      pkg_install \
        gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
        gst-plugins-ugly gst-libav gst-plugin-pipewire libnice pipewire \
        openh264 libva-utils intel-media-driver \
        cargo-c git curl base-devel pkgconf openssl xdg-desktop-portal
      ;;
    zypper)
      # NOTE on openSUSE package names (verified against the Tumbleweed OSS repo):
      #  - The GStreamer ICE plugin that provides `nicesink` is `gstreamer-libnice`
      #    (a separate package); the bare `libnice` library alone does NOT ship the
      #    GStreamer element, so it must be listed explicitly.
      #  - Core build headers (gstreamer-1.0.pc) come from `gstreamer-devel`; the
      #    app/audio/video .pc files gstreamer-rs needs come from
      #    `gstreamer-plugins-base-devel`. There is NO `libgstreamer-1_0-0-devel`.
      #  - VA driver + openh264 are optional accel/codec extras (tolerated if absent).
      pkg_install \
        gstreamer gstreamer-plugins-base gstreamer-plugins-good \
        gstreamer-plugins-bad gstreamer-plugins-ugly gstreamer-plugins-libav \
        gstreamer-plugin-pipewire gstreamer-libnice libnice pipewire \
        gstreamer-devel gstreamer-plugins-base-devel \
        cargo-c git curl gcc gcc-c++ make pkg-config libopenssl-devel \
        libva libva-utils intel-media-driver xdg-desktop-portal
      # openh264 (software H.264) is optional; name varies, tolerate if absent.
      # (On openSUSE full VA-API codec support often needs the Packman repo; the
      # verify step never *requires* hardware accel, so a missing driver only
      # affects the informational "Hardware H.264 encoder" line.)
      pkg_install gstreamer-plugin-openh264 || warn "openh264 plugin optional"
      ;;
  esac
  ok "system packages requested (missing optional ones are tolerated)"
}

# ----------------------------------------------------------------------------
# 2. Rust toolchain (needed to build webrtcsink + Qcast)
# ----------------------------------------------------------------------------
ensure_rust() {
  step "Ensuring a Rust toolchain"
  if command -v cargo >/dev/null; then
    ok "cargo present: $(cargo --version)"
    return
  fi
  info "installing Rust via rustup (no sudo, into ~/.cargo)…"
  command -v curl >/dev/null 2>&1 \
    || die "curl is required to fetch rustup but isn't installed — install it (it's in the package list; a skipped/failed system-package step may be why) and re-run"
  # Download over TLS1.2+/HTTPS-only first (the official one-liner pipes curl
  # straight into sh; we fetch to a file so the same proto guard applies and the
  # script is inspectable). rustup ships no published checksum for the bootstrap
  # shim, so we rely on the TLS-verified HTTPS transport from the official host.
  local rustup_sh
  rustup_sh="$(mktemp /tmp/rustup.XXXXXX.sh)" || die "could not create temp file"
  retry curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$rustup_sh" \
    || { rm -f "$rustup_sh"; die "could not download rustup from https://sh.rustup.rs"; }
  sh "$rustup_sh" -y --no-modify-path || { rm -f "$rustup_sh"; die "rustup install failed"; }
  rm -f "$rustup_sh"
  # rustup was run with --no-modify-path, so the current (non-login) shell won't
  # have ~/.cargo/bin yet. Source its env if present, and add the dir explicitly
  # so the rest of this run can call cargo without opening a new shell.
  # shellcheck disable=SC1091
  [[ -r "${HOME}/.cargo/env" ]] && . "${HOME}/.cargo/env"
  case ":${PATH}:" in *":${HOME}/.cargo/bin:"*) : ;; *) PATH="${HOME}/.cargo/bin:${PATH}" ;; esac
  export PATH
  command -v cargo >/dev/null || die "cargo still not on PATH after rustup (try: source \$HOME/.cargo/env, then re-run)"
  ok "installed: $(cargo --version)"
}

ensure_cargo_c() {
  step "Ensuring cargo-c (builds C-ABI GStreamer plugins from Rust)"
  if cargo cbuild --help >/dev/null 2>&1; then ok "cargo-c present"; return; fi
  info "cargo-c not packaged/working — installing via cargo (this can take a while)…"
  retry cargo install cargo-c || die "failed to install cargo-c"
  ok "cargo-c installed"
}

# ----------------------------------------------------------------------------
# 3. webrtcsink (gst-plugins-rs) — the streaming core. Build only if absent.
# ----------------------------------------------------------------------------
have_element() { gst-inspect-1.0 "$1" >/dev/null 2>&1; }

install_webrtcsink() {
  step "Installing the webrtcsink streaming plugin"
  if have_element webrtcsink; then
    ok "webrtcsink already available — skipping build"
    return
  fi

  # Some distros now package it; try that first.
  case "$PKG" in
    dnf)    pkg_install gstreamer1-plugins-rs ;;
    pacman) pkg_install gst-plugins-rs ;;
  esac
  if have_element webrtcsink; then ok "webrtcsink provided by a distro package"; return; fi

  info "building gst-plugins-rs webrtc plugin from source (ref ${GST_PLUGINS_RS_REF})…"
  mkdir -p "$BUILD_DIR"
  local src="${BUILD_DIR}/gst-plugins-rs"
  if [[ -d "$src/.git" ]]; then
    info "reusing clone at $src — refreshing to ${GST_PLUGINS_RS_REF}"
    if git -C "$src" fetch --depth 1 origin "$GST_PLUGINS_RS_REF" 2>/dev/null; then
      git -C "$src" checkout -q FETCH_HEAD || warn "could not check out FETCH_HEAD; building whatever is checked out"
    else
      warn "could not fetch ${GST_PLUGINS_RS_REF} (offline?); building the existing checkout"
    fi
  else
    # Prefer the pinned branch; fall back to the default branch only if the
    # branch clone fails (e.g. transient mirror issue), so we still get *a* build.
    retry git clone --depth 1 --branch "$GST_PLUGINS_RS_REF" \
      https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git "$src" \
      || retry git clone --depth 1 \
        https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git "$src" \
      || die "could not clone gst-plugins-rs from gitlab.freedesktop.org (check network/HTTPS access)"
  fi

  mkdir -p "$PLUGIN_DIR"
  ( cd "$src" && retry cargo cbuild --release -p gst-plugin-webrtc ) \
    || die "cargo cbuild of gst-plugin-webrtc failed — see the cargo error above (commonly a missing devel header or no C compiler)"

  # Pick the newest matching artifact (a re-run may leave several under target/).
  local so
  so="$(find "$src/target" -name 'libgstrswebrtc.so' -printf '%T@ %p\n' 2>/dev/null \
        | sort -nr | head -1 | cut -d' ' -f2-)"
  [[ -n "$so" ]] || die "cargo cbuild reported success but libgstrswebrtc.so was not found under $src/target"
  cp -f "$so" "$PLUGIN_DIR/" || die "could not copy plugin to $PLUGIN_DIR (check permissions / disk space)"
  ok "installed $(basename "$so") -> $PLUGIN_DIR"

  have_element webrtcsink \
    || die "webrtcsink still not found — is GST_PLUGIN_PATH including $PLUGIN_DIR? (it's a default user dir, so a fresh shell should pick it up)"
  ok "webrtcsink is now available"
}

# ----------------------------------------------------------------------------
# 4. Build the Qcast host binary
# ----------------------------------------------------------------------------
build_qcast() {
  (( DO_BUILD )) || { info "skipping cargo build (--no-build)"; return; }
  step "Building the Qcast host (release)"
  ( cd "$REPO_ROOT" && retry cargo build --release -p qcast-sender ) \
    || die "cargo build of qcast-sender failed"
  ok "built: ${REPO_ROOT}/target/release/qcast-sender"
}

# ----------------------------------------------------------------------------
# 5. Verification — fail loudly if anything Qcast needs at runtime is missing
# ----------------------------------------------------------------------------
verify() {
  step "Verifying the installation"
  if ! command -v gst-inspect-1.0 >/dev/null 2>&1; then
    die "gst-inspect-1.0 is not on PATH — the GStreamer base package isn't installed (or its bin dir isn't in PATH). Install GStreamer and re-run."
  fi
  local missing=0

  # Critical GStreamer elements. webrtcbin is included because the host's own
  # preflight (missing_webrtc_support) requires it alongside webrtcsink.
  local required=(webrtcsink webrtcbin videoconvert videoscale vp8enc rtpbin)
  for el in "${required[@]}"; do
    if have_element "$el"; then ok "element: $el"; else warn "MISSING element: $el"; missing=1; fi
  done

  # WebRTC transport elements (provided by libnice + GStreamer).
  for el in nicesink dtlsenc srtpenc; do
    if have_element "$el"; then ok "element: $el"; else warn "MISSING element: $el (install the libnice GStreamer plugin)"; missing=1; fi
  done

  # At least one screen-capture source (Wayland portal first, X11 fallback).
  local cap=""
  if   have_element pipewiresrc; then cap=pipewiresrc
  elif have_element ximagesrc;   then cap=ximagesrc
  fi
  if [[ -n "$cap" ]]; then
    ok "capture source: $cap"
  else
    warn "MISSING screen capture source (pipewiresrc/ximagesrc)"; missing=1
  fi

  # If webrtcsink is the thing missing but we DID build & copy the plugin, the
  # current shell most likely just hasn't picked up the user plugin dir yet.
  if ! have_element webrtcsink && [[ -e "${PLUGIN_DIR}/libgstrswebrtc.so" ]]; then
    warn "libgstrswebrtc.so is present in ${PLUGIN_DIR} but webrtcsink isn't loading."
    warn "Open a fresh shell (the user plugin dir is a GStreamer default), or set:"
    warn "  export GST_PLUGIN_PATH=\"${PLUGIN_DIR}:\${GST_PLUGIN_PATH}\""
  fi

  # (TURN relay is built into Qcast — no external coturn needed.)

  # The built binary. We normally build release, but a developer may have only a
  # debug build (cargo build without --release) — accept either so --verify is
  # useful before/without a release build.
  local rel="${REPO_ROOT}/target/release/qcast-sender"
  local dbg="${REPO_ROOT}/target/debug/qcast-sender"
  if   [[ -x "$rel" ]]; then ok "qcast-sender binary present (release)"; QCAST_BIN="$rel"
  elif [[ -x "$dbg" ]]; then ok "qcast-sender binary present (debug)";   QCAST_BIN="$dbg"
  elif (( ONLY_VERIFY )); then
    # --verify is for checking the environment; not having built yet is expected.
    warn "qcast-sender binary not built yet (run without --verify, or 'cargo build --release -p qcast-sender')"
  elif (( DO_BUILD )); then
    warn "qcast-sender binary missing after build"; missing=1
  fi

  if (( missing )); then
    die "verification found missing components (see ! lines above). Re-run after installing them, or check your distro's package names."
  fi
  printf '\n%s✔ Qcast is ready.%s  Run:  %s%s%s\n' \
    "$GRN$BOLD" "$RST" "$BOLD" "${QCAST_BIN:-${REPO_ROOT}/target/release/qcast-sender}" "$RST"
}

# ----------------------------------------------------------------------------
main() {
  detect_distro
  need_sudo
  if (( ONLY_VERIFY )); then verify; exit 0; fi
  install_system_packages
  ensure_rust
  ensure_cargo_c
  install_webrtcsink
  build_qcast
  verify
}
main
