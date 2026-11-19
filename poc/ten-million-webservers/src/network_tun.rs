use super::network::{NetworkResult, run};
use smoltcp::phy::{Medium, TunTapInterface};
use smoltcp::wire::HardwareAddress;
use std::net::Ipv4Addr;

pub(super) fn start(ip: Ipv4Addr, listeners: usize) -> NetworkResult<()> {
    run(
        TunTapInterface::new("smolbench", Medium::Ip)?,
        HardwareAddress::Ip,
        ip,
        false,
        listeners,
    )
}
