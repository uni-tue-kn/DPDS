/* Single entry-point header for bindgen: every DPDK header the `dpdk` crate needs. */

#include <stdint.h>
#include <rte_cycles.h>
#include <rte_eal.h>
#include <rte_errno.h>
#include <rte_ethdev.h>
#include <rte_launch.h>
#include <rte_lcore.h>
#include <rte_mbuf.h>
#include <rte_mbuf_dyn.h>
#include <rte_mempool.h>
#include <rte_ring.h>
#include <rte_telemetry.h>

/* rte_errno is a macro for a per-lcore TLS variable, so bindgen cannot expose
 * it; this static-inline accessor becomes a linkable symbol via wrap_static_fns. */
static inline int rte_errno_value(void) { return rte_errno; }

/* The RTE_ETH_RSS_* constants use the function-like RTE_BIT64(), which bindgen
 * cannot expand; expose the default RSS hash profile (IP/TCP/UDP) the same way. */
static inline uint64_t rte_eth_rss_ip_tcp_udp(void) {
    return RTE_ETH_RSS_IP | RTE_ETH_RSS_TCP | RTE_ETH_RSS_UDP;
}
