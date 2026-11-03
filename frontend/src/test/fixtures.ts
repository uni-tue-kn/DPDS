// Fixture builders for the API types. Deliberately distinct values so tests can tell fields apart.
import type {
  ConfigDto, Counters, DirectionPlan, DirectionStats, EmulationState, EmulationView, ExportDocument, Hint,
  HistogramView, Interface, Link, OnlineView, PortStats, Preset, Settings, StatsView,
} from '../api/types';

export function online(over: Partial<OnlineView> = {}): OnlineView {
  return { version: '0.1.0', dpds_bin: '/usr/local/bin/dpds', telemetry: true, root: true, ...over };
}

export function config(over: Partial<ConfigDto> = {}): ConfigDto {
  return {
    buffer_capacity: 1000,
    reordering: false,
    delayer: {
      distribution: 'NormalDistribution', mean_ns: 1000, std_ns: 10,
      activate_correlation: false, half_life_period_ns: 0,
    },
    limiter: { mode: 'Spacer', activate: false, rate_bps: 1e9, limit_bit: 0 },
    loss_model: { type: 'Random', p: 0 },
    seed: null,
    histogram: null,
    delay_stats: true,
    ...over,
  };
}

/** Second direction: everything different from `config()`. */
export function config2(): ConfigDto {
  return {
    buffer_capacity: 5000,
    reordering: true,
    delayer: {
      distribution: 'GammaDistribution', mean_ns: 2500, std_ns: 300,
      activate_correlation: true, half_life_period_ns: 4000000,
    },
    limiter: { mode: 'TokenBucketFilter', activate: true, rate_bps: 40000000000, limit_bit: 120000 },
    loss_model: { type: 'GilbertElliot', p: 0.01, r: 0.2, k: 0.9, h: 0.3, state_is_good: false },
    seed: 42,
    histogram: { bins: 64, lo_ns: 100, hi_ns: 9000 },
    delay_stats: false,
  };
}

/** Third config, different from `config()` and `config2()` in every field (presets). */
export function config3(): ConfigDto {
  return {
    buffer_capacity: 321,
    reordering: true,
    delayer: {
      distribution: 'LogNormalDistribution', mean_ns: 7777, std_ns: 777,
      activate_correlation: false, half_life_period_ns: 555,
    },
    limiter: { mode: 'Spacer', activate: true, rate_bps: 5000000000, limit_bit: 64000 },
    loss_model: { type: 'Random', p: 0.25 },
    seed: 7,
    histogram: { bins: 32, lo_ns: 1000, hi_ns: 50000 },
    delay_stats: true,
  };
}

export function preset(over: Partial<Preset> = {}): Preset {
  return { id: 1, name: 'wan', description: 'Typical WAN', config: config3(), ...over };
}

export function exportDoc(over: Partial<ExportDocument> = {}): ExportDocument {
  return {
    format: 'dpds-controller-export',
    version: 1,
    settings: settings(),
    links: [{ name: 'a-b', ports: ['0000:65:00.0', '0000:65:00.1'], configs: [config(), config2()], workers: null }],
    presets: [{ name: 'wan', description: 'Typical WAN', config: config3() }],
    ...over,
  };
}

export function link(over: Partial<Link> = {}): Link {
  return {
    id: 1, name: 'a-b', ports: ['0000:65:00.0', '0000:65:00.1'], configs: [config(), config2()],
    workers: null, ...over,
  };
}

export function loopLink(over: Partial<Link> = {}): Link {
  return { id: 2, name: 'loop', ports: ['net_null0'], configs: [config()], workers: null, ...over };
}

export function iface(over: Partial<Interface> = {}): Interface {
  return {
    pci: '0000:65:00.0', vendor_id: '0x15b3', device_id: '0x1017', vendor: 'Mellanox',
    driver: 'mlx5_core', netdev: 'enp101s0f0', mac: 'aa:bb:cc:00:00:01', operstate: 'up',
    speed_mbps: 100000, mtu: 1500, numa_node: 0,
    pcie_link: { speed: '16.0 GT/s PCIe', width: 16, max_speed: '16.0 GT/s PCIe', max_width: 16 },
    kernel_route: false, dpds_usable: true, reason: null, ...over,
  };
}

export function interfaces(): Interface[] {
  return [
    iface(),
    iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1', mac: 'aa:bb:cc:00:00:02' }),
    iface({
      pci: '0000:b3:00.0', vendor_id: '0x8086', device_id: '0x1572', vendor: 'Intel', driver: 'i40e',
      netdev: 'enp179s0f0', mac: 'aa:bb:cc:00:00:03', speed_mbps: 10000, numa_node: 1, pcie_link: null,
      dpds_usable: false, reason: 'driver i40e: bind to vfio-pci',
    }),
    iface({
      pci: '0000:17:00.0', netdev: 'eno1', mac: 'aa:bb:cc:00:00:04', speed_mbps: null, mtu: null,
      numa_node: null, kernel_route: true, dpds_usable: false,
      reason: 'carries a kernel route: management NIC',
    }),
  ];
}

export function hints(): Hint[] {
  return [
    { id: 'isolation', level: 'error', title: 'No core isolation', detail: 'isolcpus is not set.', suggestion: 'Add isolcpus=2-5 nohz_full=2-5 rcu_nocbs=2-5 to the kernel command line.' },
    { id: 'hugepages', level: 'warn', title: 'Few hugepages', detail: 'Only 2 free.', suggestion: 'Reserve 4 GiB of 1G hugepages.' },
    { id: 'governor', level: 'warn', title: 'Governor is powersave', detail: 'cpu2 uses powersave.', suggestion: 'Use the performance governor.' },
    { id: 'mprq', level: 'info', title: 'MPRQ is never set', detail: 'The controller never enables mprq_en.', suggestion: 'Nothing to do.' },
    { id: 'irqbalance', level: 'ok', title: 'irqbalance not running', detail: 'No irqbalance process.', suggestion: 'None.' },
  ];
}

export function settings(over: Partial<Settings> = {}): Settings {
  return {
    main_lcore: null, mlx5_devargs: 'txq_mpw_en=1', eal_extra_args: [], poll_interval_ms: 500, ...over,
  };
}

export function plan(): DirectionPlan[] {
  return [
    { dir: 0, link: 'a-b', rx: '0000:65:00.0', tx: '0000:65:00.1', lcore: 3 },
    { dir: 1, link: 'a-b', rx: '0000:65:00.1', tx: '0000:65:00.0', lcore: 4 },
  ];
}

export function emulation(state: EmulationState, over: Partial<EmulationView> = {}): EmulationView {
  return {
    state, pid: state === 'running' ? 4242 : null, started_at: state === 'running' ? 1762000000 : null,
    directions: plan(), main_lcore: 0, pending_restart: false, error: null, log_tail: [], ...over,
  };
}

export function counters(over: Partial<Counters> = {}): Counters {
  return {
    rx_pkts: 1000, dpds_rejects: 10, loss_drops: 4, limiter_drops: 3, buffer_full_drops: 3, pops: 990,
    tx_pkts: 990, tx_sent: 990, tx_drops: 0, ...over,
  };
}

export function dirStats(dir: number, over: Partial<DirectionStats> = {}): DirectionStats {
  return {
    dir, link: 'a-b', rx: dir === 0 ? '0000:65:00.0' : '0000:65:00.1', tx: dir === 0 ? '0000:65:00.1' : '0000:65:00.0',
    rx_pps: 1234567, tx_pps: 1200000, rx_bps: 9.5e9, tx_bps: 8.25e9,
    loss_pct: 0.5, limiter_pct: 1.25, buffer_full_pct: 2.5, nic_tx_pct: 0.1, nic_rx_pct: 0.75,
    delay_stats: true,
    delay_mean_ns: 1000000, delay_std_ns: 50000, delay_min_ns: 800000, delay_max_ns: 1300000,
    lateness_mean_ns: 2500, lateness_max_ns: 12000000,
    buffer_len_max: 1234,
    first_loss_reason: null, first_loss_detail: null, first_loss_relative_ns: null,
    counters: counters(), ...over,
  };
}

/** NIC port counters; every counter distinct so a table cell can be traced to its field. */
export function portStats(port: number, over: Partial<PortStats> = {}): PortStats {
  return {
    port, name: `0000:65:00.${port}`, ipackets: 1234567, opackets: 7654321, ibytes: 98765432100, obytes: 12345678900,
    imissed: 11, ierrors: 22, oerrors: 33, rx_nombuf: 44, ...over,
  };
}

export function stats(directions: DirectionStats[], over: Partial<StatsView> = {}): StatsView {
  return { directions, ports: [], age_ms: 120, stale: false, ...over };
}

/**
 * Histogram with a deliberately non-256 bin count (5). Since v8 it carries the K3 extras: percentiles (all distinct,
 * none clamped), an `expected` curve (exact model, no limiter note) and the window fields (since start).
 */
export function histogram(dir: number, over: Partial<HistogramView> = {}): HistogramView {
  return {
    dir, lo_ns: 900000, bin_ns: 100000, bins: [1, 10, 100, 40, 2 + dir], underflow: 3, overflow: 7,
    edges_ns: [900000, 1000000, 1100000, 1200000, 1300000, 1400000],
    window_s: 0, span_ms: 5000, count: 163 + dir,
    percentiles: {
      p50: 1050000, p90: 1150000, p99: 1250000, p999: 1290000,
      p50_clamped: null, p90_clamped: null, p99_clamped: null, p999_clamped: null,
    },
    expected: [2, 12.5, 90.25, 45, 5.5], expected_underflow: 2.5, expected_overflow: 6.5, expected_model: 'exact',
    ...over,
  };
}
