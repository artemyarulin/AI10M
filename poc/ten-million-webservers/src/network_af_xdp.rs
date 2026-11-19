use super::network::{NetworkResult, run};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::EthernetAddress;
use std::io;
use std::mem::size_of;
use std::net::Ipv4Addr;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use xsk_rs::config::{BindFlags, FrameSize, QueueSize, SocketConfig, UmemConfig, XdpFlags};
use xsk_rs::{CompQueue, FillQueue, FrameDesc, RxQueue, Socket, TxQueue, Umem};

// Native-copy AF_XDP, polled directly by the scheduler thread.
const BURST: usize = 32;
const RING: u32 = 2048;

struct PacketDevice {
    rx: RxQueue,
    fill: FillQueue,
    tx: Transmit,
    umem: Umem,
    received: [FrameDesc; BURST],
    head: usize,
    tail: usize,
    polls: usize,
}

struct Transmit {
    queue: TxQueue,
    completion: CompQueue,
    free: Vec<FrameDesc>,
    waiting: Vec<FrameDesc>,
    completed: [FrameDesc; BURST],
    turns: u64,
    in_flight: usize,
}

struct Rx<'a>(&'a [u8]);
struct Tx<'a>(&'a Umem, &'a mut Transmit);

impl PacketDevice {
    fn open(name: &str) -> NetworkResult<Self> {
        let ring = QueueSize::new(RING)?;
        let umem_config = UmemConfig::builder()
            .frame_size(FrameSize::new(2048)?)
            .fill_queue_size(ring)
            .comp_queue_size(ring)
            .build()?;
        let (umem, mut frames) = Umem::new(umem_config, NonZeroU32::new(RING * 2).unwrap(), false)?;
        let config = SocketConfig::builder()
            .rx_queue_size(ring)
            .tx_queue_size(ring)
            .xdp_flags(XdpFlags::XDP_FLAGS_DRV_MODE)
            .bind_flags(BindFlags::XDP_COPY | BindFlags::XDP_USE_NEED_WAKEUP)
            .build();
        let (queue, rx, rings) = unsafe { Socket::new(config, &umem, &name.parse()?, 0)? };
        let (mut fill, completion) = rings.ok_or("AF_XDP returned no UMEM rings")?;
        let free = frames.split_off(RING as usize);
        if unsafe { fill.produce(&frames) } != frames.len() {
            return Err("could not fill the AF_XDP RX ring".into());
        }

        let mut flags = 0u32;
        let mut len = size_of::<u32>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                queue.fd().as_raw_fd(),
                libc::SOL_XDP,
                libc::XDP_OPTIONS,
                (&mut flags as *mut u32).cast(),
                &mut len,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error().into());
        }
        if flags & libc::XDP_OPTIONS_ZEROCOPY != 0 {
            return Err("AF_XDP expected native copy".into());
        }

        Ok(Self {
            rx,
            fill,
            umem,
            tx: Transmit {
                queue,
                completion,
                free,
                waiting: Vec::with_capacity(RING as usize),
                completed: [frames[0]; BURST],
                turns: 0,
                in_flight: 0,
            },
            received: [frames[0]; BURST],
            head: 0,
            tail: 0,
            polls: 0,
        })
    }

    fn fetch(&mut self) -> bool {
        if self.head == self.tail {
            // The fill reservation is all-or-nothing. Keep the burst if the kernel
            // has not published enough free entries yet.
            if self.tail > 0 && unsafe { self.fill.produce(&self.received[..self.tail]) } == 0 {
                return false;
            }
            self.tail = unsafe { self.rx.consume(&mut self.received) };
            self.head = 0;
            if self.tail == 0 {
                if self.fill.needs_wakeup() {
                    self.fill
                        .wakeup(self.rx.fd_mut(), 0)
                        .expect("wake AF_XDP RX");
                }
                return false;
            }
        }
        true
    }
}

impl Transmit {
    fn flush(&mut self) {
        let count = unsafe { self.completion.consume(&mut self.completed) };
        self.free.extend_from_slice(&self.completed[..count]);
        self.in_flight -= count;

        let mut submitted = 0;
        if !self.waiting.is_empty() {
            let available = self.queue.nb_free(self.waiting.len() as u32) as usize;
            submitted = unsafe {
                self.queue
                    .produce(&self.waiting[..available.min(self.waiting.len())])
            };
            self.waiting.drain(..submitted);
            self.in_flight += submitted;
        }
        self.turns += 1;
        let retry = self.turns.is_multiple_of(64) && self.in_flight != 0;
        if (submitted != 0 || retry) && self.queue.needs_wakeup() {
            self.queue.wakeup().expect("wake AF_XDP TX");
        }
    }
}

impl Device for PacketDevice {
    type RxToken<'a> = Rx<'a>;
    type TxToken<'a> = Tx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Rx<'_>, Tx<'_>)> {
        // Keep a 32-packet batch even when RX returns several short bursts.
        if self.polls == 0 || self.tx.free.is_empty() {
            self.tx.flush();
        }
        self.polls = (self.polls + 1) % BURST;
        if self.tx.free.is_empty() || !self.fetch() {
            self.polls = 0;
            return None;
        }
        let frame = &self.received[self.head];
        self.head += 1;
        let bytes = unsafe { self.umem.data(frame) }.contents();
        Some((Rx(bytes), Tx(&self.umem, &mut self.tx)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_>> {
        (!self.tx.free.is_empty()).then_some(Tx(&self.umem, &mut self.tx))
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
        let mut frame = self.1.free.pop().unwrap();
        let mut data = unsafe { self.0.data_mut(&mut frame) };
        data.cursor().set_pos(len);
        let bytes = data.contents_mut();
        bytes.fill(0);
        let result = f(bytes);
        self.1.waiting.push(frame);
        result
    }
}

pub(super) fn start(ip: Ipv4Addr, listeners: usize) -> NetworkResult<()> {
    let nic = "enp0s2";
    let mac = std::fs::read_to_string(format!("/sys/class/net/{nic}/address"))?;
    let hardware = mac
        .trim()
        .parse::<EthernetAddress>()
        .map_err(|_| "invalid Ethernet address on enp0s2")?;
    run(
        PacketDevice::open(nic)?,
        hardware.into(),
        ip,
        false,
        listeners,
    )
}
