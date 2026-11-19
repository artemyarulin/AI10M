use super::{network_af_xdp, network_dpdk_custom, network_tap, network_tun};
use smoltcp::iface::{Config, Interface};
use smoltcp::phy::Device;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpCidr};
use std::net::Ipv4Addr;

pub(super) type NetworkResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn run<D: Device>(
    mut device: D,
    hardware: HardwareAddress,
    ip: Ipv4Addr,
    routed_tap: bool,
    listeners: usize,
) -> NetworkResult<()> {
    let (local, gateway) = if routed_tap {
        (
            Ipv4Addr::new(198, 51, 100, 2),
            Ipv4Addr::new(198, 51, 100, 1),
        )
    } else {
        let local = Ipv4Addr::new(192, 0, 2, 1);
        (local, local)
    };
    let mut config = Config::new(hardware);
    config.random_seed = 0x31038ce0;
    let mut interface = Interface::new(config, &mut device, Instant::now());
    interface.update_ip_addrs(|addrs| {
        addrs.push(IpCidr::new(local.into(), 24)).unwrap();
        if ip != local {
            addrs.push(IpCidr::new(ip.into(), 32)).unwrap();
        }
    });
    interface
        .routes_mut()
        .add_default_ipv4_route(gateway)
        .unwrap();
    interface.set_any_ip(true);
    super::scheduler::run_guests(ip, listeners, interface, device)
}

pub(super) fn start(ip: &str, backend: &str) -> NetworkResult<()> {
    let ip = ip.parse()?;
    let listeners = match std::env::var("LISTENERS") {
        Ok(count) => count.parse()?,
        Err(std::env::VarError::NotPresent) => 10_000_000,
        Err(error) => return Err(error.into()),
    };
    if listeners == 0 || (listeners - 1) as u64 > u64::from(u32::MAX - u32::from(ip)) {
        return Err("LISTENERS exceeds the available IPv4 addresses".into());
    }
    match backend {
        "tun" => network_tun::start(ip, listeners),
        "tap" => network_tap::start(ip, listeners),
        "dpdk" => network_dpdk_custom::start(ip, listeners),
        "af_xdp" => network_af_xdp::start(ip, listeners),
        _ => Err("backend must be tap, tun, dpdk, or af_xdp".into()),
    }
}
