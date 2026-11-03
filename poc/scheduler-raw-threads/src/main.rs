extern crate libc;

use std::cell::{Cell, UnsafeCell};

std::arch::global_asm!(
    r#"
.global cruncher, reporter, guests_end, enter_guest, resume_main
cruncher:
    ldr  x3, =191692582605
    mov  x2, x3
    mul  x0, x3, x3
guest_loop:
    subs x0, x0, x2
    sub  x3, x3, #1
    cbnz x3, guest_loop
    cset w0, ne
    b    resume_main
reporter:
    mov  x0, #1
    adr  x1, message
    mov  x2, #8
    mov  x8, #64
    svc  #0
    b    reporter
guests_end:
message:
    .ascii "Working\n"

enter_guest:
    stp  x29, x30, [sp, #-176]!
    stp  x19, x20, [sp, #16]
    stp  x21, x22, [sp, #32]
    stp  x23, x24, [sp, #48]
    stp  x25, x26, [sp, #64]
    stp  x27, x28, [sp, #80]
    mrs  x9, fpcr
    str  x9, [sp, #160]
    mov  x9, sp
    str  x9, [x0]
    add  x9, sp, #96
    st1  {{v8.1d-v11.1d}}, [x9], #32
    st1  {{v12.1d-v15.1d}}, [x9]
    // Restore the registers used by these two guests.
    mov  x9, x1
    mov  x10, x2
    ldp  x0, x1, [x10, #8]
    ldp  x2, x3, [x10, #24]
    ldr  x8, [x10, #72]
    ldr  x10, [x10, #264]
    msr  nzcv, x10
    br   x9
resume_main:
    ldr  x9, [sp, #160]
    msr  fpcr, x9
    add  x9, sp, #96
    ld1  {{v8.1d-v11.1d}}, [x9], #32
    ld1  {{v12.1d-v15.1d}}, [x9]
    ldp  x19, x20, [sp, #16]
    ldp  x21, x22, [sp, #32]
    ldp  x23, x24, [sp, #48]
    ldp  x25, x26, [sp, #64]
    ldp  x27, x28, [sp, #80]
    ldp  x29, x30, [sp], #176
    ret
"#
);
unsafe extern "C" {
    fn cruncher();
    fn reporter();
    fn guests_end();
    fn enter_guest(main_sp: *mut usize, pc: u64, task: *const Task) -> u64;
}

#[repr(C)]
struct Task {
    pc: u64,
    regs: [u64; 32], // x0-x30; slot 31 is unused.
    nzcv: u64,       // Arithmetic flags: Negative, Zero, Carry, Overflow.
    finished: bool,
}
impl Task {
    fn new(entry: unsafe extern "C" fn()) -> Self {
        Self {
            pc: entry as usize as u64,
            regs: [0; 32],
            nzcv: 0,
            finished: false,
        }
    }
}

struct Worker {
    tasks: UnsafeCell<Vec<Task>>,
    current: Cell<usize>,
    main_sp: Cell<usize>,
    signal_stack: UnsafeCell<[u128; 4096]>,
    timer: Cell<libc::timer_t>,
}

impl Worker {
    unsafe fn new(tasks: Vec<Task>) -> Box<Self> {
        let worker = Box::new(Self {
            current: Cell::new(tasks.len().saturating_sub(1)),
            tasks: UnsafeCell::new(tasks),
            main_sp: Cell::new(0),
            signal_stack: UnsafeCell::new([0; 4096]),
            timer: Cell::new(std::mem::zeroed()),
        });
        worker.start_timer();
        worker
    }

    unsafe fn start_timer(&self) {
        let stack = libc::stack_t {
            ss_sp: self.signal_stack.get().cast(),
            ss_flags: 0,
            ss_size: std::mem::size_of::<[u128; 4096]>(),
        };
        libc::sigaltstack(&stack, std::ptr::null_mut());
        let action = libc::sigaction {
            sa_sigaction: Self::interrupt as usize,
            sa_flags: libc::SA_SIGINFO | libc::SA_RESTART | libc::SA_ONSTACK,
            ..std::mem::zeroed()
        };
        libc::sigaction(libc::SIGALRM, &action, std::ptr::null_mut());
        let mut event: libc::sigevent = std::mem::zeroed();
        event.sigev_notify = libc::SIGEV_THREAD_ID;
        event.sigev_signo = libc::SIGALRM;
        event.sigev_value.sival_ptr = (self as *const Self).cast_mut().cast();
        event.sigev_notify_thread_id = libc::gettid();
        assert_eq!(
            libc::timer_create(libc::CLOCK_MONOTONIC, &mut event, self.timer.as_ptr()),
            0
        );
        let interval = libc::timespec {
            tv_sec: 0,
            tv_nsec: 2_000_000,
        };
        let timer = libc::itimerspec {
            it_interval: interval,
            it_value: interval,
        };
        assert_eq!(
            libc::timer_settime(self.timer.get(), 0, &timer, std::ptr::null_mut()),
            0
        );
    }

    unsafe fn next_task(&self) -> Option<usize> {
        let tasks = &*self.tasks.get();
        (1..=tasks.len())
            .map(|offset| (self.current.get() + offset) % tasks.len())
            .find(|&i| !tasks[i].finished)
    }

    unsafe fn run(&self) -> u64 {
        let mut status = 0;
        while let Some(i) = self.next_task() {
            self.current.set(i);
            let task = (&*self.tasks.get()).as_ptr().add(i);
            // Save main's return context; timer ticks switch directly between guests.
            status = enter_guest(self.main_sp.as_ptr(), (*task).pc, task);
            // A guest finished; interrupts no longer return here.
            (&mut *self.tasks.get())[self.current.get()].finished = true;
            if status != 0 {
                break;
            }
        }
        libc::timer_delete(self.timer.get());
        status
    }

    unsafe extern "C" fn interrupt(_: i32, info: *mut libc::siginfo_t, state: *mut libc::c_void) {
        // Get the CPU state Linux saved when the timer interrupted us.
        let context = &mut (*(state as *mut libc::ucontext_t)).uc_mcontext;
        // Find the worker using the pointer attached to its timer.
        let worker = &*((*info).si_value().sival_ptr as *const Self);
        // Ignore interruptions outside guest code.
        if context.pc < cruncher as usize as u64 || context.pc >= guests_end as usize as u64 {
            return;
        }
        // Save where the guest stopped, its registers and condition flags.
        let task = &mut (&mut *worker.tasks.get())[worker.current.get()];
        task.pc = context.pc;
        task.regs[..31].copy_from_slice(&context.regs);
        task.nzcv = context.pstate;
        // Choose the next unfinished guest and load it into Linux's return frame.
        let next = worker.next_task().unwrap();
        worker.current.set(next);
        let task = &(&*worker.tasks.get())[next];
        context.pc = task.pc;
        context.regs.copy_from_slice(&task.regs[..31]);
        context.pstate = (context.pstate & !0xf0000000) | (task.nzcv & 0xf0000000);
    }
}

fn main() {
    unsafe {
        let worker = Worker::new(vec![
            Task::new(cruncher),
            Task::new(cruncher),
            Task::new(cruncher),
            Task::new(cruncher),
            Task::new(cruncher),
        ]);
        std::process::exit(worker.run() as i32);
    }
}
