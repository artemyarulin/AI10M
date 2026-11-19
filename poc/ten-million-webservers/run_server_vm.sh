#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

backend=${1:-dpdk}
case "$backend" in
    dpdk|tap|tun|af_xdp) ;;
    *) echo 'Choose dpdk, tap, tun, or af_xdp.' >&2; exit 1 ;;
esac
listeners=${LISTENERS:-10000000}
[[ $listeners =~ ^[0-9]+$ ]]
ssh -tt -F ../../bin/ten-million-webservers-vm/ssh_config server \
    "sudo bash /home/ubuntu/ten-million-webservers/server.sh $backend $listeners"
