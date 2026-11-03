// Pure CPU work - subtract N from N² N times and check for zero
// Host might corrupt x0 while iterations remain to trigger exit 1.
// Estimated duration per iteration count:
// 1s 3124155503; 5s 15620777513; 10s 31241555026; 30s 95846291303; 60s 191692582605

.global _start
_start:
    ldr  x1, =191692582605 // Change constant to increase a duration
    mov  x2, x1           // Preserve N
    mul  x0, x1, x1       // Start at N squared, modulo 2^64
loop:
    subs x0, x0, x2       // Subtract N; set the zero flag.
    sub  x1, x1, #1       // Decrement the counter; preserve flags.
    cbnz x1, loop
    cset w0, ne           // Correct result: 0. Nonzero result: 1.
    mov  x8, #93          // syscall exit(status).
    svc  #0

