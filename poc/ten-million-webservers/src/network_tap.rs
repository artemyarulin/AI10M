use super::network::{NetworkResult, run};
use smoltcp::phy::{Medium, TunTapInterface};
use smoltcp::wire::EthernetAddress;
use std::net::Ipv4Addr;

pub(super) fn start(ip: Ipv4Addr, listeners: usize) -> NetworkResult<()> {
    run(
        TunTapInterface::new("smolbench", Medium::Ethernet)?,
        EthernetAddress([0x52, 0x54, 0, 0xab, 0, 1]).into(),
        ip,
        true,
        listeners,
    )
}
