#include <arpa/inet.h>
#include <asm/unistd.h>
#include <sys/socket.h>

/* 
Original and simple webserver that we've started with (webserver_libc.c) uses
libc for all the syscalls, so assembly instructions contains no syscalls calls.
For this PoC we don't want to go full in in LFI and recompile everything, so
as a hack we make syscalls calls directly, then compile it to Assembly and embed
it inside a Rust using global_asm with manual rewrite of syscalls. Functionality
wise it's almost the same
*/

static inline long raw_syscall(long number, long a0, long a1, long a2, long a3)
{
    register long x8 __asm__("x8") = number;
    register long x0 __asm__("x0") = a0;
    register long x1 __asm__("x1") = a1;
    register long x2 __asm__("x2") = a2;
    register long x3 __asm__("x3") = a3;
    register long x4 __asm__("x4") = 0;
    register long x5 __asm__("x5") = 0;

    __asm__ volatile("svc #0" : "+r"(x0)
                     : "r"(x8), "r"(x1), "r"(x2), "r"(x3), "r"(x4), "r"(x5)
                     : "memory", "cc");
    return x0;
}

int main(int argc, char **argv)
{
    (void)argc;
    struct sockaddr_in address = {
        .sin_family = AF_INET,
        .sin_port = htons(8080),
        .sin_addr.s_addr = inet_addr(argv[1]),
    };
    int server = raw_syscall(__NR_socket, AF_INET, SOCK_STREAM, 0, 0);
    raw_syscall(__NR_bind, server, (long)&address, sizeof address, 0);
    if (raw_syscall(__NR_listen, server, 128, 0, 0) < 0)
        return 1;

    static const char reply[] = "HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok";
    for (int client; (client = raw_syscall(__NR_accept, server, 0, 0, 0)) >= 0;) {
        if (raw_syscall(__NR_recvfrom, client, 0, 4096, MSG_TRUNC) > 0)
            raw_syscall(__NR_sendto, client, (long)reply, sizeof reply - 1, MSG_NOSIGNAL);
        raw_syscall(__NR_close, client, 0, 0, 0);
    }
}
