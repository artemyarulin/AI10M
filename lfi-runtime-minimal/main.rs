//! A tiny Linux/ARM64 host for one specific LFI guest: print(getpid()); ppoll().
//!
//! The Makefile compiles this file twice: `--cfg guest` emits the guest assembly;
//! the normal build produces the host. There is no allocator, libc, or liblfi.
//! All guests share code and a temporary host stack. Each retains just one
//! eight-byte gate pointer whose address identifies it. Packed slots replace
//! independent 4 GiB LFI windows; these terminal guests need no saved context.
//! RAM grows by ~8.016 bytes per guest including page tables on our 4 KiB VM,
//! plus fixed host/kernel overhead. This is not a general-purpose LFI runtime.

#![no_std]
#![no_main]

// This is the original, native guest. lfi-rewrite replaces each `svc` with a
// call through [x27]. The host loads the resulting ELF, not these native bytes.
#[cfg(guest)]
core::arch::global_asm!(
    r#"
    .global _start
_start:
    mov x8, #172                 // getpid(): returns this guest's synthetic PID.
    svc #0
    mov x8, #4096                // print(x0): custom number-only syscall.
    svc #0
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    mov x4, #0
    mov x8, #73                  // ppoll(NULL, 0, NULL, NULL): terminal wait.
    svc #0
"#
);

/* The same guest after lfi-rewrite (the 80 instruction bytes we actually load).
 * Rewriting happens once at build time. Every guest executes this SAME code
 * at the SAME address; the host sets x27 to a different slot before each entry.
 * Normally x27 is a 4 GiB-aligned LFI sandbox base. Here it is an eight-byte
 * slot address, with a gate pointer at [x27]; no separate 4 GiB windows exist.
 *
 * _start:
 *     mov x8, #172                  // getpid()
 *     mov x26, x30                  // Save the original guest link register.
 *     ldr x30, [x27]                // Load the host gate pointer from this slot.
 *     blr x30                       // Call gate; x30 now points at the next add.
 *     add x30, x27, w26, uxtw        // Normal LFI LR restore; OUR gate skips it.
 *     mov x8, #4096                 // print(x0), retaining getpid's result.
 *     mov x26, x30
 *     ldr x30, [x27]
 *     blr x30
 *     add x30, x27, w26, uxtw
 *     mov x0, #0
 *     mov x1, #0
 *     mov x2, #0
 *     mov x3, #0
 *     mov x4, #0
 *     mov x8, #73                   // Terminal ppoll: gate returns to host loop.
 *     mov x26, x30
 *     ldr x30, [x27]
 *     blr x30
 *     add x30, x27, w26, uxtw        // Not reached for our terminal ppoll.
 *
 * ARM64 x30 is LR (link register). blr stores its following instruction's
 * address in LR, then branches to the call target. w26 is the LOW 32 bits of
 * x26; uxtw zero-extends those bits before addition. Standard LFI restores LR
 * as sandbox_base + low_32_bits(saved_LR), using a 4 GiB-aligned sandbox base.
 * Our slot addresses are not such bases, so that formula does not apply.
 */

#[cfg(not(guest))]
mod host {
    use core::{
        arch::{asm, global_asm},
        panic::PanicInfo,
    };

    // link.ld keeps the entire host in [0x8000, 0x9000). Packing the slot array
    // immediately after it minimizes both resident pages and page-table levels.
    const SANDBOX_BASE: usize = 0x9000;
    const SLOT_BYTES: usize = 8;
    const PAGE_BYTES: usize = 4096;

    global_asm!(
        r#"
    .section .text.entry,"ax"
    .global _start
_start:
    mov x0, sp                  // Linux's initial stack: argc, argv[], envp, auxv.
    bl main
    brk #0                      // main never returns.

    .text
    .global enter
enter:
    // One reusable host call frame, not a stack/context allocation per guest.
    stp x26, x27, [sp, #-32]!
    str x30, [sp, #16]
    mov x27, x0                 // Guest identity = address of its gate slot.
    mov x30, x1                 // All guests start at the same instruction body.
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    mov x4, #0
    br x30

    .global gate
gate:
    // Rewritten calls arrive with x8=number, x0=argument, x27=identity,
    // x26=original guest LR, x30=address immediately after the rewritten blr.
    cmp x8, #73
    b.eq blocked                // Terminal wait: return to the host's next guest.
    stp x26, x30, [sp, #-16]!
    mov x2, x0                  // dispatch(identity, number, argument)
    mov x1, x8
    mov x0, x27
    bl dispatch
    ldp x26, x30, [sp], #16

    // x26 = original guest LR; x30 = address of the rewriter's following add.
    // Our x27 is a slot, not a 4 GiB-aligned base. Restore LR directly instead
    // of applying base + low32(LR), which is wrong for this packed layout.
    // Keep the two destinations separate: restore the guest's original LR,
    // but CONTINUE execution after the add. Each ARM64 instruction is 4 bytes.
    add x16, x30, #4             // Continuation = after the skipped add.
    mov x30, x26                 // Restore the original full guest LR.
    br x16                      // Resume guest; x0 retains dispatch's result.
blocked:
    mov x0, x27                 // Let the host verify which guest reached ppoll.
    ldr x30, [sp, #16]
    ldp x26, x27, [sp], #32
    ret

    .global finish
finish:
    // Retain completion evidence in registers, available through ptrace.
    mov x20, x0                 // Requested count.
    mov x21, x1                 // Completed count.
    mov x22, x2                 // Last completed PID.

    // Only low host + ALL slot/code pages survive. Drop the startup stack,
    // environment and vDSO. No stack access or Rust call is possible after this.
    mov x0, x3                  // First address beyond the retained allocation.
    mov x1, #0x1000000000000     // End of the assumed 48-bit user address space.
    sub x1, x1, x0
    mov x8, #215                // munmap(keep_end, 2^48 - keep_end).
    svc #0
    mov x9, #0
    mov sp, x9

    // One actual kernel wait holds the whole host idle. Guests have already
    // reached their terminal waits; none needs a resumable register context.
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    mov x4, #0
    mov x5, #0
    mov x8, #73
    svc #0
    mov x0, #124                // Unexpected wakeup: exit, as in the original.
    mov x8, #94
    svc #0
    brk #0
"#
    );

    unsafe extern "C" {
        fn gate();
        fn finish(count: usize, completed: usize, last_pid: usize, keep_end: usize) -> !;
    }

    fn exit() -> ! {
        unsafe {
            asm!("svc #0", in("x8") 94usize, in("x0") 121usize, options(noreturn));
        }
    }

    fn need(ok: bool) {
        if !ok {
            exit();
        }
    }

    // Linux ARM64 ABI: x8 selects a syscall; x0..x5 carry arguments; x0 returns
    // the result or a negative errno. These are HOST syscalls, unlike dispatch.
    #[inline(always)]
    unsafe fn syscall(number: usize, args: [usize; 6]) -> usize {
        let result: usize;
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") args[0] => result,
            in("x1") args[1], in("x2") args[2], in("x3") args[3],
            in("x4") args[4], in("x5") args[5],
            options(nostack),
        );
        need(result < usize::MAX - 4095);
        result
    }

    // No PID map, FD table, or allocation: identity comes directly from x27.
    #[no_mangle]
    pub unsafe extern "C" fn dispatch(sandbox: usize, number: usize, value: usize) -> usize {
        match number {
            172 => (sandbox - SANDBOX_BASE) / SLOT_BYTES + 1,
            4096 => {
                // At most 20 decimal digits plus newline, on the reusable stack.
                let mut buffer = [0u8; 21];
                buffer[20] = b'\n';
                let mut start = 20;
                let mut n = value;
                loop {
                    start -= 1;
                    buffer[start] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }
                syscall(
                    64,
                    [1, buffer.as_ptr().add(start) as usize, 21 - start, 0, 0, 0],
                );
                0
            }
            _ => exit(),
        }
    }

    #[no_mangle]
    pub unsafe extern "C" fn main(initial_sp: *const usize) -> ! {
        // Usage: runtime GUEST_ELF COUNT. Read argv directly; no std startup.
        need(*initial_sp == 3);
        let arg = *initial_sp.add(3) as *const u8;
        let mut count = 0usize;
        for i in 0..20 {
            let c = *arg.add(i);
            if c == 0 {
                break;
            }
            need(c >= b'0' && c <= b'9');
            count = count
                .checked_mul(10)
                .and_then(|n| n.checked_add((c - b'0') as usize))
                .unwrap_or_else(|| exit());
        }
        need(count >= 1 && count <= ((1usize << 48) - SANDBOX_BASE - 8192) / SLOT_BYTES);

        // Deliberately specialized ELF loading: our build puts the 80-byte
        // guest .text at file offset 0x210. The Makefile checks that contract.
        // include_bytes supplies its LENGTH; execution bytes are read from ELF.
        let fd = syscall(56, [usize::MAX - 99, *initial_sp.add(2), 0, 0, 0, 0]); // openat
        let mut code = [0u8; include_bytes!("build/guest.text").len()];
        need(
            syscall(
                67,
                [fd, code.as_mut_ptr() as usize, code.len(), 0x210, 0, 0],
            ) == code.len(),
        );
        syscall(57, [fd, 0, 0, 0, 0, 0]); // close

        let pc = SANDBOX_BASE + count * SLOT_BYTES;
        let size = (count * SLOT_BYTES + code.len() + PAGE_BYTES - 1) & !(PAGE_BYTES - 1);
        // mmap: RW, PRIVATE | ANONYMOUS | FIXED_NOREPLACE. Real, touched pages:
        // no shared physical-page aliases or untouched virtual-memory trick.
        need(syscall(222, [SANDBOX_BASE, size, 3, 0x100022, usize::MAX, 0]) == SANDBOX_BASE);
        for i in 0..count {
            let slot = (SANDBOX_BASE + i * SLOT_BYTES) as *mut u8;
            (slot as *mut usize).write_unaligned(gate as *const () as usize);
        }
        for j in 0..code.len() {
            ((pc + j) as *mut u8).write(code[j]);
        }

        syscall(226, [SANDBOX_BASE, size, 1, 0, 0, 0]); // mprotect: read-only slots.
        let text_page = pc & !(PAGE_BYTES - 1);
        // Only the final code page(s) become RX: avoid instruction-cache
        // maintenance across potentially gigabytes of slot data.
        syscall(
            226,
            [text_page, SANDBOX_BASE + size - text_page, 5, 0, 0, 0],
        );

        // Cooperative, sequential startup. A terminal guest ppoll returns to
        // this loop through `blocked`; it does not block a separate host thread.
        for i in 0..count {
            let slot = SANDBOX_BASE + i * SLOT_BYTES;
            let returned: usize;
            asm!(
                "bl enter",
                inlateout("x0") slot => returned,
                in("x1") pc,
                clobber_abi("C"),
            );
            need(returned == slot);
        }
        for fd in 0..3 {
            syscall(57, [fd, 0, 0, 0, 0, 0]);
        }
        finish(count, count, count, SANDBOX_BASE + size);
    }

    #[panic_handler]
    fn panic(_: &PanicInfo) -> ! {
        exit()
    }
}
