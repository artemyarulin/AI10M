#!/bin/bash
set -euo pipefail

vm_ssh() {
    ssh -i bin/ssh-key -p 2222 \
        -o UserKnownHostsFile=bin/known_hosts \
        -o StrictHostKeyChecking=accept-new ubuntu@127.0.0.1 "$@"
}

vm_command() {
    printf '%s\n' "$@" | nc -U bin/monitor.sock
}

vm_boot() {
    qemu-system-aarch64 \
        -machine virt \
        -accel hvf \
        -cpu host \
        -m 10G \
        -smp 2 \
        -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
        -drive file=bin/ubuntu.qcow2,format=qcow2,if=virtio \
        -drive file=bin/seed.iso,format=raw,if=virtio,readonly=on \
        -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:2222-:22 \
        -display none \
        -serial file:bin/console.log \
        -monitor unix:bin/monitor.sock,server=on,wait=off \
        "$@" \
        </dev/null &
    vm_pid=$!
    while ! vm_ssh true 2>/dev/null; do sleep 1; done
}

task-download() {
    mkdir -p bin
    curl \
        https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-arm64.img \
        -o bin/ubuntu.qcow2
}

task-vm-create() {
    mkdir -p bin/seed
    [ -f bin/ssh-key ] || ssh-keygen -q -t ed25519 -N '' -f bin/ssh-key
    echo > bin/known_hosts
    cat > bin/seed/user-data <<EOF
#cloud-config
ssh_authorized_keys:
  - $(cat bin/ssh-key.pub)
EOF
    echo 'instance-id: ai10m' > bin/seed/meta-data
    hdiutil makehybrid -iso -joliet -default-volume-name cidata \
        -o bin/seed.iso bin/seed -ov
    qemu-img resize bin/ubuntu.qcow2 40G
    vm_boot
    vm_ssh 'bash -se' <<'EOF'
sudo cloud-init status --wait
EOF
    vm_command stop 'savevm ready' quit
    wait "$vm_pid"
}

task-vm-run() {
    shift
    vm_boot -loadvm ready
    if [ -f "$1" ]; then
        vm_ssh 'bash -se' < "$1" || true
    else
        vm_ssh "$1" || true
    fi
    vm_command quit
    wait "$vm_pid"
}

case "${1:-}" in
    download) task-download "$@" ;;
    vm-create) task-vm-create "$@" ;;
    vm-run) task-vm-run "$@" ;;
esac
