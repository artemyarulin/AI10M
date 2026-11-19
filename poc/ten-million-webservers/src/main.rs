// AArch64 guest from webserver_raw_syscalls.c, with LFI-style syscall calls.
std::arch::global_asm!(
    r#"
.text
.globl enter_guest, http_guest
.p2align 2
enter_guest:
    sub  sp, sp, #80
    stp  x19, x20, [sp, #16]
    stp  x26, x27, [sp, #32]
    stp  x28, x30, [sp, #48]
    stp  x3, x1, [sp, #64]   // Syscall context and argv
    mov  x28, x0               // Current Task
    mov  x27, x2               // Address of the host gate pointer
    ldp  w19, w20, [x28, #8]
    ldr  x9, [x28]
    br   x9

http_guest:
    mov  w0, #2                // AF_INET
    mov  w1, #1                // SOCK_STREAM
    mov  x2, xzr
    bl   syscall_socket
    mov  w19, w0               // Listener fd
    ldr  x1, [sp, #72]
    ldr  x0, [x1, #8]          // argv[1]
    bl   inet_addr
    mov  w1, #2                // AF_INET
    movk w1, #0x901f, lsl #16  // Port 8080 in network byte order
    stp  w1, w0, [sp]
    str  xzr, [sp, #8]

    sxtw x0, w19
    mov  x1, sp
    mov  w2, #16               // sizeof(sockaddr_in)
    bl   syscall_bind

    sxtw x0, w19
    mov  w1, #128              // Backlog
    bl   syscall_listen
    mov  x8, x0
    mov  w0, #1
    tbnz x8, #63, guest_return

// Each iteration starts by waiting for a client.
accept_client:
    sxtw x0, w19
    mov  x1, xzr
    mov  x2, xzr
    bl   syscall_accept
    cmn  x0, #11               // -EAGAIN: retry accept when a client is ready.
    b.eq wait_accept
    tbnz w0, #31, accept_failed
    mov  w20, w0               // Client fd

    // Loop body. Read the request and reply if data arrived.
read_client:
    mov  w0, w20
    mov  x1, xzr
    mov  w2, #4096
    mov  w3, #32               // MSG_TRUNC
    mov  x4, xzr
    mov  x5, xzr
    bl   syscall_recvfrom
    cmn  x0, #11               // -EAGAIN: retry recv when data or EOF arrives.
    b.eq wait_read
    cmp  x0, #1
    b.lt close_client

    mov  w0, w20
    adr  x1, http_reply
    mov  w2, #40
    mov  w3, #16384            // MSG_NOSIGNAL
    mov  x4, xzr
    mov  x5, xzr
    bl   syscall_sendto

close_client:
    mov  w0, w20
    bl   syscall_close
    mov  w20, #-1
    b    accept_client         // End of the loop body. Wait for the next client.

wait_accept:
    adr  x30, accept_client
    b    yield_guest

wait_read:
    adr  x30, read_client
    b    yield_guest

accept_failed:
    mov  w0, wzr
guest_return:
    str  xzr, [x28]            // A zero PC marks a finished task.
    b    resume_main

yield_guest:
    str  x30, [x28]            // Resume at the instruction after the yield.
    mov  w0, wzr
resume_main:
    stp  w19, w20, [x28, #8]
    ldp  x19, x20, [sp, #16]
    ldp  x26, x27, [sp, #32]
    ldp  x28, x30, [sp, #48]
    add  sp, sp, #80
    ret

syscall_socket:
    mov  w8, #198
    b    call_host

syscall_bind:
    mov  w8, #200
    b    call_host

syscall_listen:
    mov  w8, #201
    b    call_host

syscall_accept:
    mov  w8, #202
    b    call_host

syscall_recvfrom:
    mov  w8, #207
    b    call_host

syscall_sendto:
    mov  w8, #206
    b    call_host

syscall_close:
    mov  w8, #57
    b    call_host

call_host:
    mov  x6, x8                // Pass the syscall number after x0..x5.
    ldr  x7, [sp, #64]         // Guest syscall context
    mov  x26, x30
    ldr  x30, [x27]
    blr  x30
    mov  x30, x26              // Restore the full return address in this PoC.
    ret

http_reply:
    .ascii "HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok"
"#,
    options(raw),
);

mod network;
mod network_af_xdp;
mod network_dpdk_custom;
mod network_dpdk_net;
mod network_tap;
mod network_tun;
mod scheduler;

use scheduler::Listener;
use smoltcp::socket::tcp::{Socket, State};
use smoltcp::wire::IpListenEndpoint;
use std::ffi::{c_char, c_void};
use std::net::Ipv4Addr;

const SERVER_FD: isize = 0;
const CLIENT_FD: isize = 1;

enum GuestIo<'a> {
    Startup {
        listener: Option<Listener>,
        bound: Option<IpListenEndpoint>,
    },
    Connection(&'a mut Socket<'static>),
}

type SyscallGate =
    unsafe extern "C" fn(usize, usize, usize, usize, usize, usize, usize, *mut c_void) -> isize;

unsafe extern "C" {
    fn http_guest();
    fn enter_guest(
        task: *mut scheduler::Task,
        argv: *const *const c_char,
        gate: *const SyscallGate,
        io: *mut c_void,
    ) -> i32;
}

/// # Safety
/// `io` must point to the exclusive `GuestIo` of the current `enter_guest` call.
/// Guest pointer arguments must be valid for the requested syscall.
unsafe extern "C" fn dispatch_syscall(
    _a0: usize,
    a1: usize,
    a2: usize,
    _a3: usize,
    _a4: usize,
    _a5: usize,
    number: usize,
    io: *mut c_void,
) -> isize {
    // These handlers serve the fixed, trusted assembly guest above.
    let io = unsafe { &mut *io.cast::<GuestIo<'_>>() };
    match number as libc::c_long {
        libc::SYS_socket => syscall_socket(io),
        libc::SYS_bind => syscall_bind(io, a1),
        libc::SYS_listen => syscall_listen(io, a1),
        libc::SYS_accept => syscall_accept(io),
        libc::SYS_recvfrom => syscall_recvfrom(io, a2),
        libc::SYS_sendto => syscall_sendto(io, a1, a2),
        libc::SYS_close => syscall_close(io),
        _ => -libc::ENOSYS as isize,
    }
}

fn syscall_socket(io: &mut GuestIo<'_>) -> isize {
    let GuestIo::Startup { listener, .. } = io else {
        unreachable!()
    };
    *listener = Some(Listener::new());
    SERVER_FD
}

fn syscall_bind(io: &mut GuestIo<'_>, ptr: usize) -> isize {
    let GuestIo::Startup { bound, .. } = io else {
        unreachable!()
    };
    // The guest passes its stack sockaddr_in, always AF_INET on port 8080.
    let addr = unsafe { (ptr as *const libc::sockaddr_in).read_unaligned() };
    let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
    *bound = Some((ip, u16::from_be(addr.sin_port)).into());
    0
}

fn syscall_listen(io: &mut GuestIo<'_>, backlog: usize) -> isize {
    let GuestIo::Startup { listener, bound } = io else {
        unreachable!()
    };
    listener
        .as_mut()
        .unwrap()
        .listen(bound.unwrap(), backlog as u16);
    0
}

fn syscall_accept(io: &mut GuestIo<'_>) -> isize {
    match io {
        GuestIo::Connection(socket)
            if matches!(socket.state(), State::Established | State::CloseWait) =>
        {
            CLIENT_FD
        }
        _ => -libc::EAGAIN as isize,
    }
}

fn syscall_recvfrom(io: &mut GuestIo<'_>, len: usize) -> isize {
    let GuestIo::Connection(socket) = io else {
        unreachable!()
    };
    // MSG_TRUNC with a null buffer discards the received request.
    if len == 0 || !socket.may_recv() {
        return 0;
    }
    let queued = socket.recv_queue().min(len);
    if queued == 0 {
        return -libc::EAGAIN as isize;
    }
    let mut consumed = 0;
    while consumed < queued {
        consumed += socket
            .recv(|data| {
                let n = data.len().min(queued - consumed);
                (n, n)
            })
            .unwrap();
    }
    consumed as isize
}

fn syscall_sendto(io: &mut GuestIo<'_>, ptr: usize, len: usize) -> isize {
    let GuestIo::Connection(socket) = io else {
        unreachable!()
    };
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    socket
        .send_slice(bytes)
        .map_or(-libc::EPIPE as isize, |n| n as isize)
}

fn syscall_close(io: &mut GuestIo<'_>) -> isize {
    let GuestIo::Connection(socket) = io else {
        unreachable!()
    };
    socket.close();
    0
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some((ip, backend)) = args.next().zip(args.next()) else {
        eprintln!("usage: ten-million-webservers IP tap|tun|dpdk|af_xdp");
        std::process::exit(1);
    };
    if let Err(error) = network::start(&ip, &backend) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
