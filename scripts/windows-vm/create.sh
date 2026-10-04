#!/bin/sh
# Build the Windows QA VM from nothing, unattended: Windows 11 ARM64 under QEMU with Apple's
# hypervisor, a local admin `reviewr` with auto-logon, OpenSSH with a dedicated key, Git for
# Windows, and herdr. Takes about 30 minutes, almost all of it the Windows install itself.
# Needs `brew install qemu`, about 40 GB free, and a network. See docs/qa-install.md.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
VM_DIR="${VM_DIR:-$HOME/VMs/reviewr-windows}"
KEY="$HOME/.ssh/reviewr_win_vm"
mkdir -p "$VM_DIR/answer"
cd "$VM_DIR"

[ -f "$KEY" ] || ssh-keygen -q -t ed25519 -N "" -C reviewr-windows-qa -f "$KEY"

# The virtio drivers (network) and SPICE tools, from UTM's guest-tools ISO.
if [ ! -d answer/Drivers ]; then
  curl -fsSL -o guest-tools.iso https://getutm.app/downloads/utm-guest-tools-latest.iso
  mnt="$(mktemp -d)"
  hdiutil attach -nobrowse -readonly -mountpoint "$mnt" guest-tools.iso >/dev/null
  cp -R "$mnt/Drivers" "$mnt"/utm-guest-tools-*.exe answer/
  hdiutil detach "$mnt" >/dev/null
fi
[ -f Win11_arm64.iso ] || "$here/fetch-iso.sh" Win11_arm64.iso
cp "$here/Autounattend.xml" "$here/setup.ps1" answer/
cp "$KEY.pub" answer/authorized_keys
rm -f answer.iso
hdiutil makehybrid -iso -joliet -default-volume-name REVIEWR_ANSWER -o answer.iso answer >/dev/null

qemu-img create -f qcow2 windows.qcow2 80G >/dev/null
cp "$(brew --prefix)/share/qemu/edk2-arm-vars.fd" vars.fd
"$here/run-vm.sh"
# The installer's "Press any key to boot from CD or DVD" prompt waits a few seconds.
for _ in $(seq 1 40); do printf 'sendkey spc\n' | nc -U monitor.sock >/dev/null 2>&1; sleep 0.5; done
echo "installing; watch on vnc://127.0.0.1:5905, ready once: $here/vm 'Get-Content C:\\setup-done.txt'"
