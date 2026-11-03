// Block in ppoll forever; x0-x4 must be zero on entry.
.global _start
_start:
    mov x8, #73
    svc #0
