# AI10M

LLM solves all kind of possible programming problems, so only tasks that are left for human are impossible. 

Let's try to solve that one: How to run 10 million of Codex CLI on one machine with 100GB RAM?

- Part 1 tells about LFI, sandboxes and processes overhead in Linux https://kapteko.com/running-10-million-codex-clis-on-one-server-lfi-processes/ . `lfi-runtime-minimal` is Rust LFI runtime, a throw away PoC
- Part 2 (in progress) will be about custom userland schedule and actual new LFI runtime in Rust

