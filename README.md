# AI10M

LLM solves all kind of possible programming problems, so only tasks that are left for human are impossible. 

Let's try to solve that one: How to run 10 million of Codex CLI on one machine with 100GB RAM?

- [Part 1](https://kapteko.com/running-10-million-codex-clis-on-one-server-lfi-processes/) tells about LFI, sandboxes and processes overhead in Linux. [poc/lfi-runtime-minimal](poc/lfi-runtime-minimal/) is Rust LFI runtime, a throw away PoC
- [Part 2](https://kapteko.com/userland_linux_scheduler_in_rust) tells about building custom userland schedule. Source available at [poc/scheduler-raw-threads](poc/scheduler-raw-threads/)

