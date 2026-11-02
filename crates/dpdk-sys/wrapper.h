/* Single entry-point header for bindgen.  Includes every DPDK header the
 * `dpdk` crate needs; wrap_static_fns in build.rs handles the static-inline
 * functions automatically so no manual helper declarations are required. */

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

/* rte_errno is a macro expanding to a per-lcore TLS variable, so bindgen
 * cannot expose it directly.  Wrap it in a static-inline accessor that
 * wrap_static_fns turns into a linkable symbol. */
static inline int rte_errno_value(void) { return rte_errno; }

/* The RTE_ETH_RSS_* constants are compound macros built from the
 * function-like RTE_BIT64(), which bindgen's macro evaluator cannot expand.
 * Expose the default RSS hash profile (IP/TCP/UDP) the same way as
 * rte_errno: through a static-inline accessor. */
static inline uint64_t rte_eth_rss_ip_tcp_udp(void) {
    return RTE_ETH_RSS_IP | RTE_ETH_RSS_TCP | RTE_ETH_RSS_UDP;
}
