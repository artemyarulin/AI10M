// Infinite CPU-only loop; never returns or makes a syscall.
.global _start
_start:
    b _start
