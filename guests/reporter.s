// Print "Working" forever, like the scheduler's reporter guest.
.global _start
_start:
    mov x0, #1
    adr x1, message
    mov x2, #8
    mov x8, #64
    svc #0
    b _start
message:
    .ascii "Working\n"
