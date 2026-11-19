# Ten millions of webservers

The purpose of this proof of concept is two explore limitation of extremely dense environments with millions of network services. Original idea was to explore `io_uring` as a batching mechanism for better
throughtput, but turns our using kernel network stack is too expensive for our purpose in terms of RAM usage.

So solve it we've went to userland TCP/IP stack implementation (specifically `smoltcp` Rust lib) and using kernel bypassing mechanism of different kind: from TUN/TAP to AF_XDP and most extreme DPDK. Not only it gave full control on RAM usage, but it also increased overall throughtput considerably.

Conclution of this poc is that userland network stack is the way to go, but also highly valuable would be multiple network stack implementations.

