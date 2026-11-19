// Single-threaded DPDK adapter with loaned RX mbufs and batched TX packets.
#include <rte_cycles.h>
#include <rte_eal.h>
#include <rte_ethdev.h>
#include <stdint.h>
#include <string.h>

#define OUTPUT_CAPACITY 4096

static struct rte_mempool *packets;
static struct rte_mbuf *burst[64];
static unsigned next, end;
static struct rte_mbuf *output[OUTPUT_CAPACITY];
static unsigned head, tail;
static uint64_t deadline, retry_at, delay_cycles, retry_cycles;
static const unsigned batch = RTE_DIM(burst);
static struct rte_mbuf *rx_loan, *tx_loan;

static void packetio_flush(void)
{
    unsigned count = tail - head;
    if (!count)
        return;
    uint64_t now = rte_get_timer_cycles();
    if (now < retry_at || (count < batch && now < deadline))
        return;
    unsigned offset = head % OUTPUT_CAPACITY;
    count = RTE_MIN(RTE_MIN(count, OUTPUT_CAPACITY - offset), batch);
    unsigned sent = rte_eth_tx_burst(0, 0, output + offset, count);
    head += sent;
    if (sent < count)
        retry_at = now + retry_cycles;
}

void packetio_init(uint8_t mac[6])
{
    char *args[] = {"packetio", "-l", "0", "-a", "0000:00:02.0", "--no-telemetry",
                    "--huge-unlink=always", "--iova-mode=pa", "-m", "32", NULL};
    if (rte_eal_init(10, args) < 0)
        rte_exit(1, "DPDK EAL init failed\n");

    delay_cycles = 60 * (rte_get_timer_hz() / 1000) / 1000;
    retry_cycles = rte_get_timer_hz() / 200000;
    packets = rte_pktmbuf_pool_create("packets", 4095, 0, 0, RTE_MBUF_DEFAULT_BUF_SIZE, 0);
    struct rte_eth_conf conf = {0};
    uint16_t rx = 512, tx = 512;
    struct rte_ether_addr address;
    if (!packets || rte_eth_dev_configure(0, 1, 1, &conf) < 0 ||
        rte_eth_dev_adjust_nb_rx_tx_desc(0, &rx, &tx) < 0 ||
        rte_eth_rx_queue_setup(0, 0, rx, 0, NULL, packets) < 0 ||
        rte_eth_tx_queue_setup(0, 0, tx, 0, NULL) < 0 ||
        rte_eth_dev_start(0) < 0 || rte_eth_macaddr_get(0, &address) < 0)
        rte_exit(1, "DPDK Virtio queues failed: %s\n", rte_strerror(rte_errno));
    memcpy(mac, address.addr_bytes, 6);
}

unsigned packetio_rx_view(uint8_t *scratch, unsigned size, const uint8_t **frame)
{
    if (rx_loan) {
        rte_pktmbuf_free(rx_loan);
        rx_loan = NULL;
    }
    // Service partial TX batches at polling boundaries and while RX is idle.
    if (next == end || next % 32 == 0)
        packetio_flush();
    if (next == end) {
        next = 0;
        end = rte_eth_rx_burst(0, 0, burst, batch);
        if (!end)
            return 0;
    }
    struct rte_mbuf *m = burst[next++];
    unsigned len = rte_pktmbuf_pkt_len(m);
    *frame = len <= size ? rte_pktmbuf_read(m, 0, len, scratch) : NULL;
    if (!*frame || *frame == scratch)
        rte_pktmbuf_free(m);
    else
        rx_loan = m;
    return *frame ? len : 0;
}

uint8_t *packetio_tx_begin(unsigned len)
{
    if (tail - head == OUTPUT_CAPACITY)
        return NULL;
    struct rte_mbuf *m = rte_pktmbuf_alloc(packets);
    if (!m)
        return NULL;
    uint8_t *room = (uint8_t *)rte_pktmbuf_append(m, len);
    if (!room) {
        rte_pktmbuf_free(m);
        return NULL;
    }
    tx_loan = m;
    return room;
}

void packetio_tx_end(void)
{
    if (head == tail)
        deadline = rte_get_timer_cycles() + delay_cycles;
    output[tail++ % OUTPUT_CAPACITY] = tx_loan;
    tx_loan = NULL;
    if (tail - head >= batch)
        packetio_flush();
}

void packetio_close(void)
{
    rte_pktmbuf_free(rx_loan);
    rte_pktmbuf_free(tx_loan);
    while (next < end)
        rte_pktmbuf_free(burst[next++]);
    while (head != tail)
        rte_pktmbuf_free(output[head++ % OUTPUT_CAPACITY]);
    rte_eth_dev_stop(0);
    rte_eth_dev_close(0);
    rte_mempool_free(packets);
    rte_eal_cleanup();
}
