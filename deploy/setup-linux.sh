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
need_sudo() { [[ $EUID -ne 0 ]] && SUDO="sudo"; }

pkg_install() {
  # Install a list of packages; missing-package errors are tolerated per-distro
  # because names differ across versions (we verify elements at the end anyway).
  local pkgs=("$@")
  case "$PKG" in
    dnf)    retry $SUDO dnf install -y --skip-unavailable "${pkgs[@]}" \
              || retry $SUDO dnf install -y "${pkgs[@]}" || true ;;
    apt)    retry $SUDO apt-get update -y || true
            retry $SUDO apt-get install -y --no-install-recommends "${pkgs[@]}" || true ;;
    pacman) retry $SUDO pacman -Sy --needed --noconfirm "${pkgs[@]}" || true ;;
    zypper) retry $SUDO zypper --non-interactive install -y "${pkgs[@]}" || true ;;
  esac
}

# ----------------------------------------------------------------------------
# 1. System packages: GStreamer stack, capture, TURN relay, build tools
# ----------------------------------------------------------------------------
install_system_packages() {
  step "Installing GStreamer + capture + TURN + build dependencies"
  case "$PKG" in
    dnf)
      pkg_install \
        gstreamer1 gstreamer1-plugins-base gstreamer1-plugins-good \
        gstreamer1-plugins-bad-free gstreamer1-plugins-bad-free-extras \
        gstreamer1-plugins-ugly-free gstreamer1-libav \
        gstreamer1-plugin-libav libnice libnice-gstreamer1 \
        pipewire pipewire-gstreamer \
        libva libva-utils mesa-va-drivers intel-media-driver \
        gstreamer1-devel gstreamer1-plugins-base-devel \
        cargo-c git gcc gcc-c++ make pkgconf-pkg-config openssl-devel \
        xdg-desktop-portal
      # openh264 (software H.264) lives in the fedora-cisco repo.
      pkg_install gstreamer1-plugin-openh264 || warn "openh264 plugin optional"
      ;;
    apt)
      pkg_install \
        gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav \
        gstreamer1.0-nice gstreamer1.0-pipewire libnice10 \
        pipewire libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
        libssl-dev pkg-config build-essential git \
        va-driver-all intel-media-va-driver xdg-desktop-portal
      ;;
    pacman)
      pkg_install \
        gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
        gst-plugins-ugly gst-libav gst-plugin-pipewire libnice pipewire \
        openh264 libva-utils intel-media-driver \
        cargo-c git base-devel pkgconf openssl xdg-desktop-portal
      ;;
    zypper)
      pkg_install \
        gstreamer gstreamer-plugins-base gstreamer-plugins-good \
        gstreamer-plugins-bad gstreamer-plugins-ugly gstreamer-plugins-libav \
        gstreamer-plugin-pipewire libnice2 pipewire \
        libgstreamer-1_0-0-devel cargo-c git gcc gcc-c++ make pkg-config \
        libopenssl-devel xdg-desktop-portal
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
  retry curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh \
    || die "could not download rustup"
  sh /tmp/rustup.sh -y --no-modify-path || die "rustup install failed"
  # shellcheck disable=SC1091
  source "${HOME}/.cargo/env"
  command -v cargo >/dev/null || die "cargo still not on PATH after rustup"
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
    info "reusing clone at $src"
    git -C "$src" fetch --depth 1 origin "$GST_PLUGINS_RS_REF" 2>/dev/null \
      && git -C "$src" checkout -q FETCH_HEAD || true
  else
    retry git clone --depth 1 --branch "$GST_PLUGINS_RS_REF" \
      https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git "$src" \
      || retry git clone --depth 1 \
        https://gitlab.freedesktop.org/gstreamer/gst-plugins-rs.git "$src" \
      || die "could not clone gst-plugins-rs"
  fi

  mkdir -p "$PLUGIN_DIR"
  ( cd "$src" && retry cargo cbuild --release -p gst-plugin-webrtc ) \
    || die "cargo cbuild of gst-plugin-webrtc failed"

  local so
  so="$(find "$src/target" -name 'libgstrswebrtc.so' -printf '%T@ %p\n' 2>/dev/null \
        | sort -nr | head -1 | cut -d' ' -f2-)"
  [[ -n "$so" ]] || die "build succeeded but libgstrswebrtc.so not found"
  cp -f "$so" "$PLUGIN_DIR/" || die "could not copy plugin to $PLUGIN_DIR"
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
  local missing=0

  # Critical GStreamer elements.
  local required=(webrtcsink videoconvert videoscale vp8enc rtpbin)
  for el in "${required[@]}"; do
    if have_element "$el"; then ok "element: $el"; else warn "MISSING element: $el"; missing=1; fi
  done

  # WebRTC transport elements (provided by libnice + GStreamer).
  for el in nicesink dtlsenc srtpenc; do
    if have_element "$el"; then ok "element: $el"; else warn "MISSING element: $el (install libnice GStreamer plugin)"; missing=1; fi
  done

  # At least one screen-capture source.
  if have_element pipewiresrc || have_element ximagesrc; then
    ok "capture source: $(have_element pipewiresrc && echo pipewiresrc || echo ximagesrc)"
  else
    warn "MISSING screen capture source (pipewiresrc/ximagesrc)"; missing=1
  fi

  # (TURN relay is built into Qcast — no external coturn needed.)

  # The built binary.
  if [[ -x "${REPO_ROOT}/target/release/qcast-sender" ]]; then
    ok "qcast-sender binary present"
  elif (( DO_BUILD )); then
    warn "qcast-sender binary missing"; missing=1
  fi

  if (( missing )); then
    die "verification found missing components (see ! lines above). Re-run after installing them, or check your distro's package names."
  fi
  printf '\n%s✔ Qcast is ready.%s  Run:  %s%s/target/release/qcast-sender%s\n' \
    "$GRN$BOLD" "$RST" "$BOLD" "$REPO_ROOT" "$RST"
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
