use super::network::{NetworkResult, run};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::EthernetAddress;
use std::marker::PhantomData;
use std::net::Ipv4Addr;
use std::{ptr, slice};

unsafe extern "C" {
    fn packetio_init(mac: *mut u8);
    fn packetio_rx_view(scratch: *mut u8, size: u32, frame: *mut *const u8) -> u32;
    fn packetio_tx_begin(size: u32) -> *mut u8;
    fn packetio_tx_end();
    fn packetio_close();
}

struct PacketDevice {
    scratch: [u8; 2048],
}

struct Rx<'a>(&'a [u8]);
struct Tx<'a>(PhantomData<&'a mut PacketDevice>);

impl PacketDevice {
    fn open() -> (Self, EthernetAddress) {
        let mut mac = [0; 6];
        unsafe { packetio_init(mac.as_mut_ptr()) };
        (Self { scratch: [0; 2048] }, EthernetAddress(mac))
    }
}

impl Drop for PacketDevice {
    fn drop(&mut self) {
        unsafe { packetio_close() };
    }
}

impl Device for PacketDevice {
    type RxToken<'a> = Rx<'a>;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Rx<'_>, Tx<'_>)> {
        let mut frame = ptr::null();
        let len = unsafe {
            packetio_rx_view(
                self.scratch.as_mut_ptr(),
                self.scratch.len() as u32,
                &mut frame,
            )
        };
        if len == 0 {
            return None;
        }
        // The C adapter holds the mbuf until the next receive call.
        Some((
            Rx(unsafe { slice::from_raw_parts(frame, len as usize) }),
            Tx(PhantomData),
        ))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        Some(Tx(PhantomData))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

impl RxToken for Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.0)
    }
}

impl TxToken for Tx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let room = unsafe { packetio_tx_begin(len as u32) };
        if room.is_null() {
            return f(&mut [0; 1514][..len]);
        }
        let result = unsafe {
            ptr::write_bytes(room, 0, len);
            f(slice::from_raw_parts_mut(room, len))
        };
        unsafe { packetio_tx_end() };
        result
    }
}

pub(super) fn start(ip: Ipv4Addr, listeners: usize) -> NetworkResult<()> {
    let (device, hardware) = PacketDevice::open();
    run(device, hardware.into(), ip, false, listeners)
}
