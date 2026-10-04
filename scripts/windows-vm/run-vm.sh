#!/bin/sh
# Boot the Windows QA VM headless (QEMU + Apple's hypervisor). Idempotent: a running VM is left be.
#   screen:  vnc://127.0.0.1:5905 (Screen Sharing; or vmkeys.py + a monitor screendump)
#   shell:   scripts/windows-vm/vm '<powershell>'
VM_DIR="${VM_DIR:-$HOME/VMs/reviewr-windows}"
cd "$VM_DIR" || exit 1
if [ -f qemu.pid ] && kill -0 "$(cat qemu.pid)" 2>/dev/null; then echo "already running"; exit 0; fi
P="$(brew --prefix)/share/qemu"
exec qemu-system-aarch64 \
  -name reviewr-windows \
  -machine virt,highmem=on -accel hvf -cpu host -smp 4 -m 8G \
  -drive if=pflash,format=raw,readonly=on,file="$P/edk2-aarch64-code.fd" \
  -drive if=pflash,format=raw,file=vars.fd \
  -device ramfb \
  -device qemu-xhci -device usb-kbd -device usb-tablet \
  -drive if=none,id=disk,file=windows.qcow2,format=qcow2 -device nvme,drive=disk,serial=reviewr \
  -drive if=none,id=winiso,media=cdrom,readonly=on,file=Win11_arm64.iso -device usb-storage,drive=winiso \
  -drive if=none,id=answer,media=cdrom,readonly=on,file=answer.iso -device usb-storage,drive=answer \
  -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:2222-:22 \
  -display none -vnc 127.0.0.1:5 \
  -monitor unix:monitor.sock,server,nowait \
  -daemonize -pidfile qemu.pid
