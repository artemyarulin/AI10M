use super::{GuestIo, SyscallGate, dispatch_syscall, enter_guest, http_guest};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{Socket, SocketBuffer, State};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{
    EthernetFrame, EthernetProtocol, IpListenEndpoint, IpProtocol, Ipv4Packet, TcpPacket,
};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::CString;
use std::net::Ipv4Addr;
use std::ptr;

const PORT: u16 = 8080;
const BUFFER_BYTES: usize = 64;
const MAX_CONNECTIONS: usize = 1_000_000;
const MAX_TIME_WAIT: usize = 65_536;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct Task {
    pc: u64,        // Instruction address to return to
    fd_server: i32, // Server file descriptor
    fd_client: i32, // Client file descriptor
}

// enter_guest accesses these fields at fixed AArch64 assembly offsets.
const _: () = {
    assert!(std::mem::size_of::<Task>() == 16);
    assert!(std::mem::offset_of!(Task, pc) == 0);
    assert!(std::mem::offset_of!(Task, fd_server) == 8);
    assert!(std::mem::offset_of!(Task, fd_client) == 12);
};

impl Task {
    fn new(entry: unsafe extern "C" fn()) -> Self {
        Self {
            pc: entry as *const () as u64,
            fd_server: -1,
            fd_client: -1,
        }
    }

    fn resume(&mut self, io: &mut GuestIo<'_>, ip: Option<Ipv4Addr>) -> i32 {
        let address = ip.map(|ip| CString::new(ip.to_string()).unwrap());
        let argv = [
            ptr::null(),
            address.as_ref().map_or(ptr::null(), |ip| ip.as_ptr()),
            ptr::null(),
        ];
        let gate: SyscallGate = dispatch_syscall;
        unsafe { enter_guest(self, argv.as_ptr(), &gate, ptr::from_mut(io).cast()) }
    }
}

pub(super) struct Listener {
    sockets: SocketSet<'static>,
    backlog: u16,
    pending: u16,
}

impl Listener {
    pub(super) fn new() -> Self {
        Self {
            sockets: new_socket_set(),
            backlog: 0,
            pending: 0,
        }
    }

    pub(super) fn listen(&mut self, endpoint: IpListenEndpoint, backlog: u16) {
        listen_on(&mut self.sockets, endpoint);
        self.backlog = backlog;
    }
}

fn new_socket_set() -> SocketSet<'static> {
    let mut sockets = SocketSet::new(Vec::with_capacity(1));
    sockets.add(Socket::new(
        SocketBuffer::new(vec![0; BUFFER_BYTES]),
        SocketBuffer::new(vec![0; BUFFER_BYTES]),
    ));
    sockets
}

fn listen_on(sockets: &mut SocketSet<'static>, endpoint: IpListenEndpoint) {
    let socket = sockets.get_mut::<Socket>(SocketHandle::default());
    socket.listen(endpoint).unwrap();
    socket.set_timeout(Some(Duration::from_secs(300)));
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Flow {
    listener: usize,
    remote: Ipv4Addr,
    port: u16,
}

// Host lifecycle: waiting for accept, running the guest, then finishing TCP close.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pending,
    Queued,
    Reading,
    Finished,
    Acked,
    Retained,
}

struct Connection {
    sockets: SocketSet<'static>,
    stage: Stage,
    wake_at: Option<Instant>,
}

struct Worker {
    tasks: Vec<Task>,
    listeners: Vec<Listener>,
    interface: Interface,
    first: u32,
    connections: HashMap<Flow, Connection>,
    ready: HashMap<usize, VecDeque<Flow>>,
    timers: BTreeMap<Instant, Vec<Flow>>,
    empty: SocketSet<'static>,
    time_wait: usize,
}

impl Worker {
    fn start(&mut self) {
        for (index, task) in self.tasks.iter_mut().enumerate() {
            let ip = Ipv4Addr::from(self.first + index as u32);
            let mut io = GuestIo::Startup {
                listener: None,
                bound: None,
            };
            assert_eq!(task.resume(&mut io, Some(ip)), 0);
            let GuestIo::Startup {
                listener: Some(listener),
                ..
            } = io
            else {
                unreachable!()
            };
            let socket = listener.sockets.get::<Socket>(SocketHandle::default());
            assert_ne!(task.pc, 0);
            assert_eq!(task.fd_client, -1);
            assert!(socket.is_listening());
            assert_eq!(socket.listen_endpoint(), (ip, PORT).into());
            self.listeners.push(listener);
        }
        let count = self.listeners.len();
        println!(
            "resident_listeners_verified={count} rx_bytes={BUFFER_BYTES} tx_bytes={BUFFER_BYTES}"
        );
        println!("ready listeners={count}");
    }

    fn incoming(&self, frame: &[u8], medium: Medium) -> Option<(Flow, bool)> {
        let ip = ipv4_packet(frame, medium)?;
        if ip.version() != 4
            || ip.next_header() != IpProtocol::Tcp
            || ip.more_frags()
            || ip.frag_offset() != 0
        {
            return None;
        }
        let listener = u32::from(ip.dst_addr()).checked_sub(self.first)? as usize;
        if listener >= self.listeners.len() {
            return None;
        }
        let tcp = TcpPacket::new_checked(ip.payload()).ok()?;
        if tcp.dst_port() != PORT {
            return None;
        }
        let flow = Flow {
            listener,
            remote: ip.src_addr(),
            port: tcp.src_port(),
        };
        Some((flow, tcp.syn() && !tcp.ack()))
    }

    fn ingress<T: TxToken>(
        &mut self,
        now: Instant,
        frame: &[u8],
        tx: T,
        capabilities: &DeviceCapabilities,
    ) -> Option<Flow> {
        let target = self.incoming(frame, capabilities.medium);
        let mut packet = Packet {
            frame: Some(frame),
            tx: Some(tx),
            capabilities: capabilities.clone(),
            acked: false,
        };
        if let Some((flow, syn)) = target {
            if let Some(connection) = self.connections.get_mut(&flow) {
                self.interface
                    .poll_ingress_single(now, &mut packet, &mut connection.sockets);
                if connection.stage == Stage::Finished
                    && packet.acked
                    && matches!(
                        connection
                            .sockets
                            .get::<Socket>(SocketHandle::default())
                            .state(),
                        State::TimeWait | State::Closing
                    )
                {
                    connection.stage = Stage::Acked;
                }
                return Some(flow);
            }
            if syn {
                let listener = &mut self.listeners[flow.listener];
                if listener.pending >= listener.backlog || self.connections.len() >= MAX_CONNECTIONS
                {
                    return None;
                }

                self.interface
                    .poll_ingress_single(now, &mut packet, &mut listener.sockets);
                if listener
                    .sockets
                    .get::<Socket>(SocketHandle::default())
                    .state()
                    != State::SynReceived
                {
                    return None;
                }

                // The resident socket consumed the SYN. Keep its TCP state and
                // leave a listening socket at the guest's address for the next SYN.
                let mut replacement = new_socket_set();
                let ip = Ipv4Addr::from(self.first + flow.listener as u32);
                listen_on(&mut replacement, (ip, PORT).into());
                let sockets = std::mem::replace(&mut listener.sockets, replacement);
                listener.pending += 1;
                self.connections.insert(
                    flow,
                    Connection {
                        sockets,
                        stage: Stage::Pending,
                        wake_at: None,
                    },
                );
                return Some(flow);
            }
        }
        self.interface
            .poll_ingress_single(now, &mut packet, &mut self.empty);
        None
    }

    fn advance(&mut self, flow: Flow, now: Instant, device: &mut impl Device) {
        let Some(connection) = self.connections.get_mut(&flow) else {
            return;
        };
        let socket = connection
            .sockets
            .get_mut::<Socket>(SocketHandle::default());
        let task = &mut self.tasks[flow.listener];
        let listener = &mut self.listeners[flow.listener];
        let established = matches!(socket.state(), State::Established | State::CloseWait);
        let resume = match connection.stage {
            Stage::Pending => established && task.fd_client == -1,
            Stage::Reading => socket.can_recv() || !socket.may_recv(),
            _ => false,
        };
        if resume {
            assert_eq!(task.resume(&mut GuestIo::Connection(socket), None), 0);
            assert!(task.pc != 0);
            if connection.stage == Stage::Pending {
                listener.pending -= 1;
            }
            connection.stage = if task.fd_client == -1 {
                Stage::Finished
            } else {
                Stage::Reading
            };
        } else if connection.stage == Stage::Pending && established {
            self.ready.entry(flow.listener).or_default().push_back(flow);
            connection.stage = Stage::Queued;
        }

        // Each SocketSet has one TCP socket, so one TX token is enough per poll.
        let mut packet = Packet {
            frame: None,
            capabilities: device.capabilities(),
            tx: device.transmit(now),
            acked: false,
        };
        self.interface
            .poll_egress(now, &mut packet, &mut connection.sockets);
        let socket = connection.sockets.get::<Socket>(SocketHandle::default());
        let state = socket.state();
        if connection.stage == Stage::Finished
            && packet.acked
            && matches!(state, State::TimeWait | State::Closing)
        {
            connection.stage = Stage::Acked;
        }
        let mut overflow = false;
        if state == State::TimeWait && connection.stage == Stage::Acked {
            overflow = self.time_wait == MAX_TIME_WAIT;
            if !overflow {
                self.time_wait += 1;
                connection.stage = Stage::Retained;
            }
        } else if state != State::TimeWait && connection.stage == Stage::Retained {
            self.time_wait -= 1;
            connection.stage = Stage::Acked;
        }
        if overflow || (state == State::Closed && connection.stage != Stage::Reading) {
            if matches!(connection.stage, Stage::Pending | Stage::Queued) {
                listener.pending -= 1;
            }
            if connection.stage == Stage::Queued {
                let queue = self.ready.get_mut(&flow.listener).unwrap();
                queue.retain(|queued| *queued != flow);
                if queue.is_empty() {
                    self.ready.remove(&flow.listener);
                }
            }
            self.connections.remove(&flow);
            return;
        }

        let deadline = if socket.state() == State::Closed {
            // TCP can close during egress while the guest is blocked in recv.
            Some(now)
        } else {
            self.interface.poll_at(now, &connection.sockets)
        };
        if let Some(deadline) = deadline {
            let deadline = deadline.max(now + Duration::from_millis(1));
            if connection
                .wake_at
                .is_none_or(|scheduled| deadline < scheduled)
            {
                connection.wake_at = Some(deadline);
                self.timers.entry(deadline).or_default().push(flow);
            }
        }
    }

    fn drive(&mut self, flow: Flow, now: Instant, device: &mut impl Device) {
        self.advance(flow, now, device);
        while self.tasks[flow.listener].fd_client == -1 {
            let Some(queue) = self.ready.get_mut(&flow.listener) else {
                break;
            };
            let next = queue.pop_front().unwrap();
            if queue.is_empty() {
                self.ready.remove(&flow.listener);
            }
            if let Some(connection) = self.connections.get_mut(&next) {
                connection.stage = Stage::Pending;
                self.advance(next, now, device);
            }
        }
    }

    fn run(&mut self, mut device: impl Device) -> ! {
        let capabilities = device.capabilities();
        loop {
            let now = Instant::now();
            self.interface.poll_maintenance(now);
            for _ in 0..32 {
                let Some((rx, tx)) = device.receive(now) else {
                    break;
                };
                if let Some(flow) = rx.consume(|frame| self.ingress(now, frame, tx, &capabilities))
                {
                    self.drive(flow, now, &mut device);
                }
            }
            while let Some((&deadline, _)) = self.timers.first_key_value() {
                if deadline > now {
                    break;
                }
                for flow in self.timers.pop_first().unwrap().1 {
                    if let Some(connection) = self.connections.get_mut(&flow)
                        && connection.wake_at == Some(deadline)
                    {
                        connection.wake_at = None;
                        self.drive(flow, now, &mut device);
                    }
                }
            }
        }
    }
}

pub(super) fn run_guests(
    ip: Ipv4Addr,
    count: usize,
    interface: Interface,
    device: impl Device,
) -> ! {
    let mut worker = Worker {
        tasks: vec![Task::new(http_guest); count],
        listeners: Vec::with_capacity(count),
        interface,
        first: ip.into(),
        connections: HashMap::new(),
        ready: HashMap::new(),
        timers: BTreeMap::new(),
        empty: SocketSet::new(Vec::new()),
        time_wait: 0,
    };
    worker.start();
    worker.run(device)
}

fn ipv4_packet(frame: &[u8], medium: Medium) -> Option<Ipv4Packet<&[u8]>> {
    let bytes = match medium {
        Medium::Ip => frame,
        Medium::Ethernet => {
            let frame = EthernetFrame::new_checked(frame).ok()?;
            if frame.ethertype() != EthernetProtocol::Ipv4 {
                return None;
            }
            frame.payload()
        }
    };
    Ipv4Packet::new_checked(bytes).ok()
}

// Present one routed packet to smoltcp and observe its outgoing ACK.
// This same adapter also handles egress, with no incoming frame.
struct Packet<'a, T> {
    frame: Option<&'a [u8]>,
    tx: Option<T>,
    capabilities: DeviceCapabilities,
    acked: bool,
}

struct Rx<'a>(&'a [u8]);
struct Tx<'a, T> {
    token: T,
    acked: &'a mut bool,
    medium: Medium,
}

impl<T: TxToken> Device for Packet<'_, T> {
    type RxToken<'a>
        = Rx<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = Tx<'a, T>
    where
        Self: 'a;

    fn receive(&mut self, now: Instant) -> Option<(Rx<'_>, Tx<'_, T>)> {
        let frame = self.frame.take()?;
        Some((Rx(frame), self.transmit(now)?))
    }

    fn transmit(&mut self, _: Instant) -> Option<Tx<'_, T>> {
        Some(Tx {
            token: self.tx.take()?,
            acked: &mut self.acked,
            medium: self.capabilities.medium,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        self.capabilities.clone()
    }
}

impl RxToken for Rx<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.0)
    }
}

impl<T: TxToken> TxToken for Tx<'_, T> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let Tx {
            token,
            acked,
            medium,
        } = self;
        token.consume(len, |frame| {
            let result = f(frame);
            if let Some(tcp) = ipv4_packet(frame, medium)
                .filter(|ip| ip.next_header() == IpProtocol::Tcp)
                .and_then(|ip| TcpPacket::new_checked(ip.payload()).ok())
                && tcp.ack()
                && !tcp.syn()
                && !tcp.fin()
                && !tcp.rst()
                && tcp.payload().is_empty()
            {
                *acked = true;
            }
            result
        })
    }
}
