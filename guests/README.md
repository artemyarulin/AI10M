# Guests

ARM64 Linux guests used in the runtime and scheduler experiments.

## Building

```sh
make build FILE=[GUEST_FILE]
```

The Makefile accepts `.s` and `.c` files and writes executables to `../bin/`. To add a new source file, just add a new file, no Makefile changes needed.

## Guests

- [busy_loop.s](busy_loop.s) — Infinite CPU-only loop
- [exit.s](exit.s) — Immediately exits with status 0
- [hello.s](hello.s) — Prints `Hello` once and exits with status 0
- [number_cruncher.s](number_cruncher.s) — Finite CPU work, exits 0 on success or 1 on failure. Corrupting x0 during the loop make guest to exit with error
- [number_cruncher_duration.c](number_cruncher_duration.c) — Same, but in C, plus elapsed wall-time output
- [reporter.s](reporter.s) — Prints `Working` forever
- [smallest_waiter.s](smallest_waiter.s) — Starts, calls `ppoll` and blocks forever
