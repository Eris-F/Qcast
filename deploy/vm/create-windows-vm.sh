#!/usr/bin/env bash
# create-windows-vm.sh — stand up a clean Windows VM on this Fedora box so Qcast's
# Windows build + installer can be built and VALIDATED (the step that needs real
# Windows). Pairs with windows-setup.ps1: once the guest has OpenSSH, Claude can SSH
# in and run deploy/tauri/build-windows.ps1 autonomously.
#
# PREREQS — run these yourself (no passwordless sudo for Claude):
#   sudo dnf install -y @virtualization        # qemu-kvm, libvirt, virt-install, virt-viewer
#   sudo systemctl enable --now libvirtd
#   sudo usermod -aG libvirt "$USER"            # then log out / back in
# and download:
#   - a Windows 10/11/25H2 x64 ISO            -> set WIN_ISO=
#   - (recommended) the virtio-win ISO         -> set VIRTIO_ISO= (fast virtio disk/net + drivers)
#     https://fedorapeople.org/groups/virt/virtio-win/direct-downloads/stable-virtio/virtio-win.iso
#
# Then:  WIN_ISO=~/Downloads/Win.iso VIRTIO_ISO=~/Downloads/virtio-win.iso deploy/vm/create-windows-vm.sh
#
# Uses the SYSTEM libvirt (qemu:///system) so the guest gets a NAT IP reachable from
# the host (needed for SSH-in). NOTE: authored blind — review/adjust before running.
set -euo pipefail

WIN_ISO="${WIN_ISO:?set WIN_ISO=/path/to/Windows.iso}"
VIRTIO_ISO="${VIRTIO_ISO:-}"
NAME="${NAME:-qcast-win}"
RAM_MB="${RAM_MB:-8192}"
VCPUS="${VCPUS:-4}"
DISK_GB="${DISK_GB:-80}"
DISK="${DISK:-/var/lib/libvirt/images/${NAME}.qcow2}"

[ -e /dev/kvm ] || { echo "ERROR: /dev/kvm missing — enable virtualization (VT-x/AMD-V) in BIOS."; exit 1; }
command -v virt-install >/dev/null || { echo "ERROR: virt-install not found — run the @virtualization install above."; exit 1; }

sudo qemu-img create -f qcow2 "$DISK" "${DISK_GB}G"

disks=(--disk "path=$DISK,format=qcow2,bus=virtio")
[ -n "$VIRTIO_ISO" ] && disks+=(--disk "path=$VIRTIO_ISO,device=cdrom")

# Interactive install (robust — no autounattend fragility). virtio disk/net need the
# virtio-win drivers loaded during Windows setup (point setup at the virtio CD).
sudo virt-install \
  --connect qemu:///system \
  --name "$NAME" \
  --memory "$RAM_MB" \
  --vcpus "$VCPUS" \
  --cpu host \
  --os-variant win11 \
  "${disks[@]}" \
  --cdrom "$WIN_ISO" \
  --network network=default,model=virtio \
  --graphics spice --video qxl \
  --boot uefi \
  --features smm.state=on

cat <<EOF

VM '$NAME' created and booting (a virt-viewer window should open).
1. Install Windows (load the virtio storage driver from the virtio CD if the disk
   isn't seen). Create a user you'll hand to Claude.
2. Inside the guest, run elevated:
     powershell -ExecutionPolicy Bypass -File deploy\\vm\\windows-setup.ps1 -EnableSsh
3. Get the guest IP from the host:
     sudo virsh -c qemu:///system domifaddr $NAME
4. Tell Claude that IP + the username so it can SSH in and run
   deploy/tauri/build-windows.ps1 to build + validate the installer.
EOF
