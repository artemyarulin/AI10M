use super::network::NetworkResult;
use dpdk_net::api::rte::{
    eal::Eal,
    eth::{EthDevBuilder, RxQueueConf, TxQueueConf},
    pktmbuf::{MemPool, MemPoolConfig},
    queue::{RxQueue, TxQueue},
};
use dpdk_net::device::DpdkDevice;
use smoltcp::wire::{EthernetAddress, HardwareAddress};
use std::sync::Arc;

#[expect(dead_code, reason = "Alternative DPDK adapter retained for comparison")]
pub(super) fn with_device<T>(
    use_device: impl FnOnce(DpdkDevice, HardwareAddress) -> T,
) -> NetworkResult<T> {
    let _eal = Eal::init([
        "ten-million-webservers",
        "-l",
        "0",
        "-a",
        "0000:00:02.0",
        "--no-telemetry",
        "--huge-unlink=always",
        "--iova-mode=pa",
        "-m",
        "32",
    ])?;
    let pool = Arc::new(MemPool::create(
        "packets",
        &MemPoolConfig::new().num_mbufs(4095).cache_size(0),
    )?);
    let port = EthDevBuilder::new(0)
        .rx_queue_conf(RxQueueConf::new().nb_desc(512))
        .tx_queue_conf(TxQueueConf::new().nb_desc(512))
        .build(&pool)?;
    let result = port.mac_addr().map(|mac| {
        let device = DpdkDevice::new(
            RxQueue::new(0, 0),
            TxQueue::new(0, 0),
            Arc::clone(&pool),
            1514,
            2048,
        );
        use_device(device, EthernetAddress(mac.addr_bytes).into())
    });
    port.stop()?;
    port.close()?;
    Ok(result?)
}
