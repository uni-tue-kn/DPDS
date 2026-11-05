// Fixture builders for the API types. Deliberately distinct values so tests can tell fields apart.
import type {
  ConfigDto,
  Counters,
  DirectionPlan,
  DirectionStats,
  EmulationState,
  EmulationView,
  Hint,
  HistogramView,
  HistorySample,
  HistoryTotals,
  HistoryView,
  Interface,
  Link,
  LinkInput,
  OnlineView,
  RunningConfig,
  StatsView,
} from '../api/types';

export function online(over: Partial<OnlineView> = {}): OnlineView {
  return { version: '1.0.0', dpds_bin: '/usr/local/bin/dpds', telemetry: true, root: true, ...over };
}

export function config(over: Partial<ConfigDto> = {}): ConfigDto {
  return {
    buffer_capacity: 1000,
    reordering: false,
    delayer: {
      distribution: 'NormalDistribution',
      mean_ns: 1000,
      std_ns: 10,
      activate_correlation: false,
      half_life_period_ns: 0,
      initialization_rate_pps: 12500000,
    },
    limiter: null, // v22: stage not added (the list reads "none")
    loss_model: { type: 'Random', p: 0 },
    seed: null,
    histogram: null,
    delay_stats: true,
    ...over,
  };
}

/** v22: no stage added at all: Buffer and Statistics only. */
export function configBare(over: Partial<ConfigDto> = {}): ConfigDto {
  return {
    buffer_capacity: 1000,
    reordering: false,
    delayer: null,
    limiter: null,
    loss_model: null,
    seed: null,
    histogram: null,
    delay_stats: true,
    ...over,
  };
}

// v12: every delayer carries `initialization_rate_pps` (required), with a distinct value per config; none of them is
// the old hard-coded 15 Mpps, so a UI that falls back to a constant instead of using the stored value is caught.

/** Second config: everything different from `config()`. */
export function config2(): ConfigDto {
  return {
    buffer_capacity: 5000,
    reordering: false, // spec45: reordering excludes correlation, which this config uses
    delayer: {
      distribution: 'GammaDistribution',
      mean_ns: 2500,
      std_ns: 300,
      activate_correlation: true,
      half_life_period_ns: 4000000,
      initialization_rate_pps: 8000000,
    },
    limiter: { mode: 'TokenBucketFilter', rate_bps: 40000000000 },
    loss_model: { type: 'GilbertElliot', p: 0.01, r: 0.2, k: 0.9, h: 0.3 },
    seed: 42,
    histogram: { bins: 64, lo_ns: 100, hi_ns: 9000 },
    delay_stats: false,
  };
}

/** Third config, different from `config()` and `config2()` in every field (a link of its own). */
export function config3(): ConfigDto {
  return {
    buffer_capacity: 321,
    reordering: true,
    delayer: {
      distribution: 'LogNormalDistribution',
      mean_ns: 7777,
      std_ns: 777,
      activate_correlation: false,
      half_life_period_ns: 555,
      initialization_rate_pps: 2500000,
    },
    limiter: { mode: 'Spacer', rate_bps: 5000000000 }, // display-only fixture: never saved through the editor (spec45)
    loss_model: { type: 'Random', p: 0.25 },
    seed: 7,
    histogram: { bins: 32, lo_ns: 1000, hi_ns: 50000 },
    delay_stats: true,
  };
}

/** One directed link (spec v10): ingress port -> egress port with one config. */
export function link(over: Partial<Link> = {}): Link {
  return {
    id: 1,
    name: 'a-b',
    ingress: '0000:65:00.0',
    egress: '0000:65:00.1',
    config: config(),
    ...over,
  };
}

/** The reverse direction of `link()` as its own link, with the "everything different" config. */
export function linkRev(over: Partial<Link> = {}): Link {
  return {
    id: 3,
    name: 'b-a',
    ingress: '0000:65:00.1',
    egress: '0000:65:00.0',
    config: config2(),
    ...over,
  };
}

/** Hairpin: ingress == egress. */
export function loopLink(over: Partial<Link> = {}): Link {
  return { id: 2, name: 'loop', ingress: 'net_null0', egress: 'net_null0', config: config(), ...over };
}

export function iface(over: Partial<Interface> = {}): Interface {
  return {
    pci: '0000:65:00.0',
    vendor_id: '0x15b3',
    device_id: '0x1017',
    vendor: 'Mellanox',
    driver: 'mlx5_core',
    netdev: 'enp101s0f0',
    mac: 'aa:bb:cc:00:00:01',
    operstate: 'up',
    speed_mbps: 100000,
    mtu: 1500,
    numa_node: 0,
    pcie_link: { speed: '16.0 GT/s PCIe', width: 16, max_speed: '16.0 GT/s PCIe', max_width: 16 },
    uplink: false,
    dpds_usable: true,
    reason: null,
    ...over,
  };
}

export function interfaces(): Interface[] {
  return [
    iface(),
    iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1', mac: 'aa:bb:cc:00:00:02' }),
    iface({
      pci: '0000:b3:00.0',
      vendor_id: '0x8086',
      device_id: '0x1572',
      vendor: 'Intel',
      driver: 'i40e',
      netdev: 'enp179s0f0',
      mac: 'aa:bb:cc:00:00:03',
      speed_mbps: 10000,
      numa_node: 1,
      pcie_link: null,
      dpds_usable: false,
      reason: 'driver i40e: bind to vfio-pci',
    }),
    iface({
      pci: '0000:17:00.0',
      netdev: 'eno1',
      mac: 'aa:bb:cc:00:00:04',
      speed_mbps: null,
      mtu: null,
      numa_node: null,
      uplink: true,
      dpds_usable: false,
      reason: 'eno1 carries a kernel route (host uplink); DPDK would take it away from the host',
    }),
  ];
}

export function hints(): Hint[] {
  return [
    {
      id: 'isolation',
      level: 'error',
      title: 'No core isolation',
      detail: 'isolcpus is not set.',
      suggestion: 'Add isolcpus=2-5 nohz_full=2-5 rcu_nocbs=2-5 to the kernel command line.',
    },
    {
      id: 'hugepages',
      level: 'warn',
      title: 'Few hugepages',
      detail: 'Only 2 free.',
      suggestion: 'Reserve 4 GiB of 1G hugepages.',
    },
    {
      id: 'governor',
      level: 'warn',
      title: 'Governor is powersave',
      detail: 'cpu2 uses powersave.',
      suggestion: 'Use the performance governor.',
    },
    {
      id: 'mtu',
      level: 'info',
      title: 'MTU is 1500',
      detail: 'The ports use the default MTU.',
      suggestion: 'Nothing to do.',
    },
    {
      id: 'irq_affinity',
      level: 'ok',
      title: 'NIC IRQs are off the isolated cores',
      detail: 'No NIC interrupt is placed on an isolated core.',
      suggestion: 'None.',
    },
  ];
}

/**
 * Spec 33: a hint that suggests shell commands for an administrator (`commands` set). DPDS never runs them; the page
 * shows them in a tooltip and can copy them. The commands are what the controller computes from the host it read.
 */
export function commandsHint(over: Partial<Hint> = {}): Hint {
  return {
    id: 'swappiness',
    level: 'info',
    title: 'vm.swappiness is 60',
    detail: 'Current value 60, target 0.',
    suggestion: 'Set vm.swappiness to 0.',
    commands: ['sysctl -w vm.swappiness=0'],
    ...over,
  };
}

export function plan(): DirectionPlan[] {
  return [
    { dir: 0, link: 'a-b', rx: '0000:65:00.0', tx: '0000:65:00.1', lcore: 3 },
    { dir: 1, link: 'b-a', rx: '0000:65:00.1', tx: '0000:65:00.0', lcore: 4 },
  ];
}

export function emulation(state: EmulationState, over: Partial<EmulationView> = {}): EmulationView {
  return {
    state,
    pid: state === 'running' ? 4242 : null,
    started_at: state === 'running' ? 1762000000 : null,
    directions: plan(),
    main_lcore: 0,
    config: null,
    error: null,
    log_tail: [],
    ...over,
  };
}

export function counters(over: Partial<Counters> = {}): Counters {
  return {
    rx_pkts: 1000,
    dpds_rejects: 10,
    loss_drops: 4,
    limiter_drops: 3,
    buffer_full_drops: 3,
    pops: 990,
    tx_pkts: 990,
    tx_sent: 990,
    tx_drops: 0,
    // spec 35 (C2), spec 47 (C4): cumulative NIC loss counters of the ingress (RX) and egress (TX) port
    nic_rx_missed: 0,
    nic_rx_nombuf: 0,
    nic_rx_port_buffer_discards: 0,
    nic_rx_phy_discards: 0,
    nic_tx_phy_discards: 0,
    // spec 47 (C4): diagnostics, in no loss total
    nic_rx_crc_errors: 0,
    nic_rx_symbol_errors: 0,
    nic_rx_length_errors: 0,
    // spec 51: frames received by the link per size bucket (P4TG's 8: 0-63, 64, 65-127, 128-255, 256-511, 512-1023, 1024-1518,
    // 1519+), all distinct, summing to the 1000 received packets; and the cumulative out-of-order departures (T1)
    frame_sizes: [10, 20, 30, 40, 100, 150, 250, 400],
    out_of_order: 7,
    ...over,
  };
}

export function dirStats(dir: number, over: Partial<DirectionStats> = {}): DirectionStats {
  return {
    dir,
    link: dir === 0 ? 'a-b' : 'b-a',
    rx: dir === 0 ? '0000:65:00.0' : '0000:65:00.1',
    tx: dir === 0 ? '0000:65:00.1' : '0000:65:00.0',
    rx_pps: 1234567,
    tx_pps: 1200000,
    rx_bps: 9.5e9,
    tx_bps: 8.25e9,
    loss_pct: 0.5,
    limiter_pct: 1.25,
    buffer_full_pct: 2.5,
    nic_tx_pct: 0.1,
    nic_rx_pct: 0.75,
    delay_stats: true,
    delay_mean_ns: 1000000,
    delay_std_ns: 50000,
    delay_min_ns: 800000,
    delay_max_ns: 1300000,
    lateness_mean_ns: 2500,
    lateness_max_ns: 12000000,
    buffer_len_max: 1234,
    first_loss_reason: null,
    first_loss_detail: null,
    first_loss_relative_ns: null,
    counters: counters(),
    ...over,
  };
}

export function stats(directions: DirectionStats[], over: Partial<StatsView> = {}): StatsView {
  return { directions, age_ms: 120, stale: false, ...over };
}

/**
 * Histogram with a deliberately non-256 bin count (5). Since v8 it carries the K3 extras: percentiles (all distinct,
 * none clamped), an `expected` curve (exact model, no limiter note) and the window fields (since start).
 */
export function histogram(dir: number, over: Partial<HistogramView> = {}): HistogramView {
  return {
    dir,
    lo_ns: 900000,
    bin_ns: 100000,
    bins: [1, 10, 100, 40, 2 + dir],
    underflow: 3,
    overflow: 7,
    edges_ns: [900000, 1000000, 1100000, 1200000, 1300000, 1400000],
    window_s: 0,
    span_ms: 5000,
    count: 163 + dir,
    percentiles: {
      p50: 1050000,
      p90: 1150000,
      p99: 1250000,
      p999: 1290000,
      p50_clamped: null,
      p90_clamped: null,
      p99_clamped: null,
      p999_clamped: null,
    },
    expected: [2, 12.5, 90.25, 45, 5.5],
    expected_underflow: 2.5,
    expected_overflow: 6.5,
    expected_model: 'exact',
    ...over,
  };
}

/**
 * Spec 51 (C2): `GET /api/stats/{dir}/iat_histogram`, the delay histogram's shape without the `expected*` fields. Deliberately
 * different from `histogram()` in every number (5 bins of 2000 ns from 0, so an IAT chart cannot be mistaken for the delay one):
 * `count` is 91 + dir (the bins 87 + dir plus the 4 overflow), the percentiles are distinct and none is clamped.
 */
export function iatHistogram(dir: number, over: Partial<HistogramView> = {}): HistogramView {
  return {
    dir,
    lo_ns: 0,
    bin_ns: 2000,
    bins: [5, 50, 20, 10, 2 + dir],
    underflow: 0,
    overflow: 4,
    edges_ns: [0, 2000, 4000, 6000, 8000, 10000],
    count: 91 + dir,
    percentiles: {
      p50: 3000,
      p90: 5000,
      p99: 7000,
      p999: 9000,
      p50_clamped: null,
      p90_clamped: null,
      p99_clamped: null,
      p999_clamped: null,
    },
    ...over,
  };
}

/**
 * What `GET /api/emulation` reports as `config` while an emulation is starting/running: the accepted start request
 * (links in start-body shape) plus the controller's own extras (assigned id/dir, planned lcore), which the frontend must
 * ignore. Defaults to `fx.link()` / `fx.linkRev()` (named a-b and b-a).
 */
export function runningConfig(links: LinkInput[] = [link(), linkRev()]): RunningConfig {
  return {
    links: links.map((l, i) => ({
      id: 100 + i,
      dir: i,
      lcore: 3 + i,
      name: l.name,
      ingress: l.ingress,
      egress: l.egress,
      config: l.config,
    })),
  };
}

/** `fx.emulation('running')` that reports `config` (default: `runningConfig()`). */
export function runningEmulation(
  config: RunningConfig = runningConfig(),
  state: EmulationState = 'running',
): EmulationView {
  return emulation(state, { config });
}

/** Spec: the rate chart plots L1 (rx_gbps + rx_mpps * 0.16); tests about something else than the L1 overhead set both Mpps to 0, so L1 = L2 and the plotted value is the rx_gbps given. */
export const NO_PPS = { rx_mpps: 0, tx_mpps: 0 } as const;

/**
 * Spec 29: one sample of `GET /api/stats/history` (`t_ms`: whole milliseconds since the run started). The defaults are the rates of `dirStats()` in the units of the
 * series (Gbit/s, Mpps) and cumulative drop counts (spec 43), so a chart built from the history shows what the stats cards show.
 */
export function historySample(t_ms: number, over: Partial<HistorySample> = {}): HistorySample {
  return {
    t_ms,
    rx_gbps: 9.5,
    tx_gbps: 8.25,
    rx_mpps: 1.234567,
    tx_mpps: 1.2,
    // spec 43 (B1): the link's cumulative drop counters at this sample, distinct per cause (they sum to 100)
    loss_drops: 40,
    limiter_drops: 30,
    buffer_full_drops: 20,
    nic_tx_drops: 5,
    nic_rx_missed: 4,
    nic_rx_nombuf: 1,
    // spec 47: the three new causes are 0 by default (the six above keep their counts; tests of the new ones set them)
    nic_rx_port_buffer_discards: 0,
    nic_rx_phy_discards: 0,
    nic_tx_phy_discards: 0,
    ...over,
  };
}

/**
 * Spec 29b: the `links` of `GET /api/stats/history`, in link-index order: the same links, names and ports as `dirStats(0)`,
 * `dirStats(1)` (the PCI addresses of `interfaces()`, which the dashboard shows as netdev names).
 */
export function historyLinks(): HistoryView['links'] {
  return [
    { name: 'a-b', ingress: '0000:65:00.0', egress: '0000:65:00.1' },
    { name: 'b-a', ingress: '0000:65:00.1', egress: '0000:65:00.0' },
  ];
}

/**
 * `GET /api/stats/history`: the run's id (spec 29b: `run_id`, the identity of a run; the first run of a controller is 1), its
 * start (unix seconds, as `emulation('running')` has it), its links and the samples per link index.
 */
export function history(samples: Record<string, HistorySample[]>, over: Partial<HistoryView> = {}): HistoryView {
  return { run_id: 1, started_at: 1762000000, links: historyLinks(), totals: {}, samples, ...over };
}

/**
 * Spec 35 (C1): one link's final cumulative counters, as `totals` of `GET /api/stats/history` carries them. Distinct counts
 * that sum to 100 drops (loss 40, limiter 30, buffer 20, NIC TX 5, missed 4, nombuf 1; the three spec 47 causes 0) out of 10000 received packets, so the
 * share of a cause "of all drops" is its count in percent, and differs from its share of the received packets.
 */
export function totals(over: Partial<HistoryTotals> = {}): HistoryTotals {
  return {
    rx_pkts: 10000,
    loss_drops: 40,
    limiter_drops: 30,
    buffer_full_drops: 20,
    nic_tx_drops: 5,
    nic_rx_missed: 4,
    nic_rx_nombuf: 1,
    // spec 47: the three new causes are 0 by default (the six above keep their counts; tests of the new ones set them)
    nic_rx_port_buffer_discards: 0,
    nic_rx_phy_discards: 0,
    nic_tx_phy_discards: 0,
    // spec 51: the forwarded (sent) frames: rx_pkts 10000 less the 100 drops above; frames received per size bucket (sum =
    // rx_pkts 10000) and the cumulative out-of-order departures
    tx_sent: 9900,
    frame_sizes: [100, 200, 300, 400, 1000, 1500, 2500, 4000],
    out_of_order: 9,
    ...over,
  };
}

/** `count` samples one second apart, at t_ms = from * 1000, (from + 1) * 1000, ... (`from` in seconds), all with the same values. */
export function historySamples(from: number, count: number, over: Partial<HistorySample> = {}): HistorySample[] {
  return Array.from({ length: count }, (_, i) => historySample((from + i) * 1000, over));
}

/** Spec 43 B1: the cumulative drop fields of a `HistorySample`, in the order of the donut's causes (and of the loss chart's lines): nine causes (spec 47). */
export const DROP_FIELDS = [
  'loss_drops',
  'limiter_drops',
  'buffer_full_drops',
  'nic_tx_drops',
  'nic_rx_missed',
  'nic_rx_nombuf',
  'nic_rx_port_buffer_discards',
  'nic_rx_phy_discards',
  'nic_tx_phy_discards',
] as const;
