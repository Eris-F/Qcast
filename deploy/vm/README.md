# Windows VM — the path to actually building + validating the installer

The Qcast Windows installer and the Tauri app **can't be built on this Fedora box**
(no MSVC GStreamer to link against; Tauri needs Windows). They need a real Windows
machine. This dir stands one up as a **VM** so the build can be done — and, with
OpenSSH on, driven **autonomously from the Fedora host**.

> You already have a dual-boot Windows too: the fastest one-off finish is to **boot
> it** and run `deploy/tauri/build-windows.ps1`. The VM exists so the build can be
> **automated/repeated** (and driven by Claude) without rebooting.

## No-sudo path (verified on this box)

`/dev/kvm` is world-accessible here (mode `0666`) and `flatpak` is installed, so a VM
needs **no `sudo`** — only a Windows ISO. GNOME Boxes (flatpak, user-level) can do an
**unattended** Windows install from an ISO (it generates the answer file + loads
virtio), then `windows-setup.ps1` makes it build-ready:

```bash
flatpak remote-add --if-not-exists --user flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y flathub org.gnome.Boxes
# then: Boxes → New → select the Windows ISO → it offers Express/unattended install
```

So the only thing blocking an end-to-end build here is **a Windows ISO**: drop one in
`~/Downloads` and Claude can take it from there with no sudo. The `virt-install` flow
below is the alternative if you prefer system libvirt.

## Flow

1. **You** (Claude has no passwordless sudo): install virt tooling + get media —
   the prereqs are commented at the top of `create-windows-vm.sh`
   (`sudo dnf install @virtualization`, a Windows x64 ISO, the virtio-win ISO).
2. `WIN_ISO=… VIRTIO_ISO=… deploy/vm/create-windows-vm.sh` — creates + boots the VM.
3. Install Windows (interactive), then run **`windows-setup.ps1 -EnableSsh`** elevated
   in the guest — it installs the full build toolchain (VS C++ Build Tools, Rust MSVC,
   GStreamer 1.26 MSVC runtime+devel, cargo-c, cargo-tauri, node) and turns on OpenSSH.
4. Get the guest IP (`sudo virsh -c qemu:///system domifaddr qcast-win`) and hand it +
   the username to Claude.
5. **Claude** then SSHes in and runs `cargo build -p qcast-sender` (first real compile
   of the SendInput injector) → scaffolds the Tauri app (`deploy/tauri/README.md`) →
   `deploy/tauri/build-windows.ps1` → validates per `deploy/WINDOWS_INSTALLER.md` §9 +
   `deploy/TEST_PLAN.md`. That closes out "finish the installer".

## Files
| File | Purpose |
| --- | --- |
| `create-windows-vm.sh` | `virt-install` a clean Windows VM (system libvirt, NAT IP, virtio). |
| `windows-setup.ps1` | In-guest: install the build toolchain + (optionally) enable OpenSSH. Also useful on the dual-boot. |

**Status:** authored on Linux, **untested** — review before running. They’re the
fastest path from "blocked on Windows" to a built, validated installer.
