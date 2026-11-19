#!/usr/bin/env bash
# Provision both VMs once; run the server and client scripts in separate terminals.
set -euo pipefail
cd "$(dirname "$0")"
repo=$(cd ../.. && pwd)
state=$repo/bin/ten-million-webservers-vm
guest=/home/ubuntu/ten-million-webservers
mkdir -p "$state"/{cache,server,client,seed}

[[ -f $state/cache/ubuntu.img ]] || curl -fL \
    https://cloud-images.ubuntu.com/noble/20261017/noble-server-cloudimg-arm64.img \
    -o "$state/cache/ubuntu.img"
[[ -f $state/ssh-key ]] || ssh-keygen -q -t ed25519 -N '' -f "$state/ssh-key"
cat > "$state/ssh_config" <<SSH
Host server client
    HostName 127.0.0.1
    User ubuntu
    IdentityFile "$state/ssh-key"
    IdentitiesOnly yes
    BatchMode yes
    StrictHostKeyChecking accept-new
    UserKnownHostsFile "$state/known_hosts"
Host server
    Port 2238
Host client
    Port 2239
SSH
ssh_vm() { ssh -F "$state/ssh_config" "$@"; }

# Both machines use the same login key and interface names.
cat > "$state/seed/user-data" <<CLOUD
#cloud-config
ssh_authorized_keys:
  - $(cat "$state/ssh-key.pub")
CLOUD
echo 'instance-id: ten-million-webservers' > "$state/seed/meta-data"
cat > "$state/seed/network-config" <<NET
version: 2
ethernets:
  enp0s1: {dhcp4: true}
  enp0s2: {dhcp4: false, optional: true}
NET
[[ -f $state/seed.iso ]] || hdiutil makehybrid -iso -joliet \
    -default-volume-name cidata -o "$state/seed.iso" "$state/seed"


create_vm() {
    local role=$1 cores=1 port=2238 octet=1 link=on
    if [[ $role == client ]]; then cores=3; port=2239; octet=2; link=off; fi
    [[ -f $state/$role/disk.qcow2 ]] || qemu-img create -f qcow2 -F qcow2 \
        -b "$state/cache/ubuntu.img" "$state/$role/disk.qcow2" 40G
    [[ $role != server ]] || rm -f "$state/link.sock"
    # Keep the device-tree hardware description used by our minimal kernel.
    qemu-system-aarch64 -daemonize -machine virt,gic-version=3,iommu=smmuv3,acpi=off \
        -accel hvf -cpu host -m 10G -smp "$cores" \
        -bios /opt/homebrew/share/qemu/edk2-aarch64-code.fd \
        -drive "file=$state/$role/disk.qcow2,format=qcow2,if=virtio,discard=unmap" \
        -drive "file=$state/seed.iso,format=raw,if=virtio,readonly=on" \
        -netdev "user,id=mgmt,hostfwd=tcp:127.0.0.1:$port-:22" \
        -device virtio-net-pci,netdev=mgmt,mac=52:54:00:12:34:56 \
        -netdev "stream,id=bench,server=$link,addr.type=unix,addr.path=$state/link.sock" \
        -device "virtio-net-pci,netdev=bench,mac=52:54:00:ab:00:0$octet,disable-legacy=on,iommu_platform=off,rx_queue_size=256,tx_queue_size=256" \
        -display none -monitor none -serial "file:$state/$role/console.log" \
        -pidfile "$state/$role/qemu.pid"
    until ssh_vm "$role" true 2>/dev/null; do sleep 1; done
}

configure_vm() {
    COPYFILE_DISABLE=1 tar --no-xattrs -cf - Cargo.toml build.rs src kernel.config -C "$repo" Cargo.lock |
        ssh_vm "$1" "mkdir -p '$guest'; tar -mxf - -C '$guest'"
    # Run this same guest setup on both VMs.
    ssh_vm "$1" "cd '$guest'; bash -se" <<'GUEST'
set -euo pipefail
sudo cloud-init status --wait
sudo apt-get update -qq
sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    iproute2 ethtool curl ca-certificates build-essential git clang-18 llvm-18 libclang1-18 \
    libdpdk-dev libbpf-dev libelf-dev zlib1g-dev libnuma-dev libpcap-dev libsystemd-dev \
    autoconf automake libtool pkg-config m4 bc bison flex libssl-dev xz-utils
sudo systemctl disable --now apt-daily.timer apt-daily-upgrade.timer
sudo swapoff -a
sudo apt-get clean

# Install both the server and the standard HTTP client on each VM.
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs |
    sh -s -- -y --profile minimal --default-toolchain 1.99.1
CLANG=clang-18 LLC=llc-18 LIBCLANG_PATH=/usr/lib/llvm-18/lib \
    ~/.cargo/bin/cargo +1.99.1 build --release --locked
curl -fL https://github.com/hatoo/oha/releases/download/v1.17.0/oha-linux-arm64 -o /tmp/oha
sudo install -m 755 /tmp/oha /usr/local/bin/oha

# Build and install the kernel inside the VM; QEMU keeps using normal UEFI boot.
mkdir -p kernel-build
sudo mount -t tmpfs -o size=4G,uid=1000 tmpfs kernel-build
curl -fL https://cdn.kernel.org/pub/linux/kernel/v7.x/linux-7.3.4.tar.xz |
    tar -xJ -C kernel-build
make -C kernel-build/linux-7.3.4 KCONFIG_ALLCONFIG="$PWD/kernel.config" allnoconfig
make -C kernel-build/linux-7.3.4 -j"$(nproc)" Image
sudo install -m 644 kernel-build/linux-7.3.4/arch/arm64/boot/Image /boot/vmlinuz-ai10m
sudo install -m 644 kernel-build/linux-7.3.4/.config /boot/config-ai10m
sudo umount kernel-build
sudo tee /boot/grub/grub.cfg >/dev/null <<'GRUB'
set timeout=0
menuentry 'AI10M' {
    linux /vmlinuz-ai10m root=/dev/vda1 rootwait rw console=ttyAMA0 iommu.passthrough=1
}
GRUB

# Ubuntu already expanded the fixed-size disk during its first boot.
echo 'resize_rootfs: false' | sudo tee /etc/cloud/cloud.cfg.d/99-ai10m.cfg >/dev/null
sudo rm -f /etc/cloud/cloud-init.disabled

# Restore the client's network namespace at boot. Its NIC has MAC suffix 02.
sudo mkdir -p /var/lib/cloud/scripts/per-boot
sudo tee /var/lib/cloud/scripts/per-boot/ten-million-network >/dev/null <<'NETWORK'
#!/usr/bin/env bash
set -euo pipefail
[[ $(cat /sys/class/net/enp0s2/address) == 52:54:00:ab:00:02 ]] || exit 0
sysctl -qw net.ipv4.tcp_child_ehash_entries=2097152
ip netns add smol-client
sysctl -qw net.ipv4.tcp_child_ehash_entries=0
ip link set enp0s2 netns smol-client
ip -n smol-client -batch - <<'NET'
link set lo up
link set enp0s2 up
addr add 192.0.2.2/24 dev enp0s2
neigh replace 192.0.2.1 lladdr 52:54:00:ab:00:01 nud permanent dev enp0s2
route replace 10.0.0.0/7 via 192.0.2.1 dev enp0s2
NET
ip netns exec smol-client sysctl -qw net.ipv4.tcp_max_tw_buckets=65536
ip netns exec smol-client sh -c 'echo 6 > /sys/class/net/enp0s2/queues/rx-0/rps_cpus'
NETWORK
sudo chmod +x /var/lib/cloud/scripts/per-boot/ten-million-network

# The server start command selects and configures its packet backend.
cat > server.sh <<'SERVER'
set -euo pipefail
cd /home/ubuntu/ten-million-webservers
backend=$1
driver=virtio-pci
[[ $backend != dpdk ]] || driver=vfio-pci
pci=/sys/bus/pci/devices/0000:00:02.0
echo 0000:00:02.0 > "$pci/driver/unbind"
echo "$driver" > "$pci/driver_override"
echo 0000:00:02.0 > /sys/bus/pci/drivers_probe
udevadm settle
if [[ $backend == dpdk ]]; then
    echo Y > /sys/module/vfio_iommu_type1/parameters/allow_unsafe_interrupts
    sysctl -qw vm.nr_hugepages=24
else
    sysctl -qw vm.nr_hugepages=0
    ip link set enp0s2 up
    ip link set enp0s2 xdp off
    if [[ $backend == af_xdp ]]; then
        ethtool -K enp0s2 gro off
        export LIBXDP_SKIP_DISPATCHER=1
    else
        ethtool -K enp0s2 gro on
        ip tuntap del dev smolbench mode tap 2>/dev/null || true
        ip tuntap del dev smolbench mode tun 2>/dev/null || true
        ip tuntap add dev smolbench mode "$backend"
        ip link set smolbench up
        sysctl -qw net.ipv4.ip_forward=1 net.ipv4.conf.all.rp_filter=0 \
            net.ipv4.conf.enp0s2.rp_filter=0 net.ipv4.conf.smolbench.rp_filter=0
        ip addr replace 192.0.2.254/24 dev enp0s2
        ip neigh replace 192.0.2.2 lladdr 52:54:00:ab:00:02 nud permanent dev enp0s2
        via=()
        if [[ $backend == tap ]]; then
            ip addr replace 198.51.100.1/24 dev smolbench
            ip link set smolbench address 52:54:00:ab:00:03
            ip neigh replace 198.51.100.2 lladdr 52:54:00:ab:00:01 nud permanent dev smolbench
            via=(via 198.51.100.2)
        fi
        ip route replace 10.0.0.0/7 "${via[@]}" dev smolbench
    fi
fi
ulimit -l unlimited
exec env LISTENERS="$2" \
    ./target/release/ten-million-webservers 10.64.0.1 "$backend" > >(tee server.log) 2>&1
SERVER
sudo reboot
GUEST
    until [[ $(ssh_vm "$1" uname -r 2>/dev/null) == 7.3.4-afxdp ]]; do sleep 1; done
    ssh_vm "$1" 'sudo cloud-init status --wait'
}

for vm in server client; do
    create_vm "$vm"
    configure_vm "$vm"
done
echo 'Ready: server 1 CPU / 10 GiB, client 3 CPUs / 10 GiB.'
echo 'Run ./run_server_vm.sh dpdk, then ./run_client_vm.sh in another terminal.'
