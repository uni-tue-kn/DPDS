// Wire types of the dpds-controller REST API (JSON names exactly as on the wire).
//
// Types only, no runtime code: shared by the tests and the implementation.

export type EmulationState = 'stopped' | 'starting' | 'running' | 'stopping' | 'failed';

export type Distribution =
  | 'DeterministicDistribution'
  | 'LogNormalDistribution'
  | 'NormalDistribution'
  | 'GammaDistribution'
  | 'UniformDistribution';

export type LimiterMode = 'Spacer' | 'TokenBucketFilter';

export interface DelayerDto {
  distribution: Distribution;
  mean_ns: number;
  std_ns: number;
  activate_correlation: boolean;
  half_life_period_ns: number;
  /** Rate (packets/s) the delay correlation assumes while it initializes; always required. */
  initialization_rate_pps: number;
}

export interface LimiterDto {
  mode: LimiterMode;
  rate_bps: number;
}

export type LossModelDto =
  | { type: 'Random'; p: number }
  | { type: 'GilbertElliot'; p: number; r: number; k: number; h: number };

/** All three optional (null/absent = automatic default: 256 bins, mean +- 6 std). */
export interface HistogramDto {
  bins?: number | null;
  lo_ns?: number | null;
  hi_ns?: number | null;
}

export interface ConfigDto {
  buffer_capacity: number;
  reordering: boolean;
  /** A stage that is null or absent is not added (the controller maps it to a no-op). */
  delayer?: DelayerDto | null;
  limiter?: LimiterDto | null;
  loss_model?: LossModelDto | null;
  seed?: number | null;
  histogram?: HistogramDto | null;
  /** default true. false = the direction records no delay statistics / histogram. */
  delay_stats?: boolean;
}

/** One direction: packets arriving on `ingress` leave on `egress` (DPDK device names) with `config` applied. */
export interface Link {
  id: number;
  name: string;
  ingress: string;
  egress: string;
  config: ConfigDto;
  /** Worker lcore; null/absent = automatic. */
  worker?: number | null;
  /** UI only, never sent to the controller: L2 frame size incl. FCS (bytes) that converts pps to bit/s. Absent = unset. */
  avg_frame_size_byte?: number | null;
}

export type LinkInput = Omit<Link, 'id'>;

/** A link as Start sends it: without the UI-only members. */
export type StartLink = Omit<LinkInput, 'avg_frame_size_byte'>;

export interface PcieLink {
  speed: string | null;
  width: number | null;
  max_speed: string | null;
  max_width: number | null;
}

export interface Interface {
  pci: string;
  vendor_id: string | null;
  device_id: string | null;
  vendor: string | null;
  driver: string | null;
  netdev: string | null;
  mac: string | null;
  operstate: string | null;
  speed_mbps: number | null;
  mtu: number | null;
  numa_node: number | null;
  pcie_link: PcieLink | null;
  /** The host uses this interface (route, IP address or Open vSwitch port on it or a device above it). */
  uplink: boolean;
  dpds_usable: boolean;
  reason: string | null;
}

export type HintLevel = 'ok' | 'info' | 'warn' | 'error';

export interface Hint {
  id: string;
  level: HintLevel;
  title: string;
  detail: string;
  suggestion: string;
}

export interface Settings {
  main_lcore?: number | null;
  eal_extra_args: string[];
  poll_interval_ms: number;
}

export interface OnlineView {
  version: string;
  dpds_bin: string;
  telemetry: boolean;
  root: boolean;
}

export interface DirectionPlan {
  dir: number;
  link: string;
  rx: string;
  tx: string;
  lcore: number;
}

export interface EmulationView {
  state: EmulationState;
  pid?: number | null;
  /** Unix seconds. */
  started_at?: number | null;
  directions: DirectionPlan[];
  main_lcore?: number | null;
  /** The configuration the emulation was started with; null/absent while stopped or failed. */
  config?: RunningConfig | null;
  error?: string | null;
  log_tail: string[];
}

export interface Counters {
  rx_pkts: number;
  dpds_rejects: number;
  loss_drops: number;
  limiter_drops: number;
  buffer_full_drops: number;
  pops: number;
  tx_pkts: number;
  tx_sent: number;
  tx_drops: number;
}

export interface DirectionStats {
  dir: number;
  link?: string | null;
  rx?: string | null;
  tx?: string | null;
  rx_pps: number;
  tx_pps: number;
  rx_bps: number;
  tx_bps: number;
  /** Drop shares in percent of the packets received in the window. */
  loss_pct: number;
  limiter_pct: number;
  buffer_full_pct: number;
  nic_tx_pct: number;
  nic_rx_pct: number;
  /** false = no delay fields (they are null) and no histogram. */
  delay_stats: boolean;
  delay_mean_ns?: number | null;
  delay_std_ns?: number | null;
  delay_min_ns?: number | null;
  delay_max_ns?: number | null;
  lateness_mean_ns?: number | null;
  lateness_max_ns?: number | null;
  buffer_len_max?: number | null;
  first_loss_reason?: string | null;
  first_loss_detail?: string | null;
  first_loss_relative_ns?: number | null;
  counters: Counters;
}

export interface PortStats {
  port: number;
  name?: string | null;
  ipackets: number;
  opackets: number;
  ibytes: number;
  obytes: number;
  imissed: number;
  ierrors: number;
  oerrors: number;
  rx_nombuf: number;
}

export interface StatsView {
  directions: DirectionStats[];
  ports: PortStats[];
  /** milliseconds since the last successful telemetry poll (null: never). */
  age_ms?: number | null;
  /** the data is old (poll failures); after 5 failures `directions` is empty. */
  stale: boolean;
  /** Milliseconds since the run started, on the controller's monotonic clock (frozen after a stop); null/absent: nothing ran. */
  elapsed_ms?: number | null;
}

/** One second of one link's run, as `GET /api/stats/history` reports it (`t_ms` is relative to the run's start). */
export interface HistorySample {
  t_ms: number;
  rx_gbps: number;
  tx_gbps: number;
  rx_mpps: number;
  tx_mpps: number;
  /** Drop shares in percent of the packets received. */
  loss_pct: number;
  limiter_pct: number;
  buffer_full_pct: number;
  nic_tx_pct: number;
  nic_rx_pct: number;
}

/** The current run's series per link index (as a string key); `started_at` (unix seconds) is null while nothing ran. */
export interface HistoryView {
  started_at: number | null;
  samples: Record<string, HistorySample[]>;
}

export type Clamped = 'underflow' | 'overflow' | null;

/** Percentiles in ns over the returned counts; null = no samples; `*_clamped` = the value is a range edge. */
export interface Percentiles {
  p50: number | null;
  p90: number | null;
  p99: number | null;
  p999: number | null;
  p50_clamped?: Clamped;
  p90_clamped?: Clamped;
  p99_clamped?: Clamped;
  p999_clamped?: Clamped;
}

/** The fields after `edges_ns` are optional so an older controller still works. */
export interface HistogramView {
  dir: number;
  lo_ns: number;
  bin_ns: number;
  /** Counts per bin (any length 1..500, not always 256). */
  bins: number[];
  underflow: number;
  overflow: number;
  /** bins.length + 1 edges. */
  edges_ns: number[];
  /** Requested window in seconds (0 = since start) and the span the counts actually cover. */
  window_s?: number;
  span_ms?: number;
  /** Samples including under/overflow. */
  count?: number;
  percentiles?: Percentiles;
  /** Expected count per bin under the configured delay distribution. */
  expected?: number[];
  expected_underflow?: number;
  expected_overflow?: number;
  expected_model?: 'exact' | 'normal_approximation';
  expected_note?: string;
}

/** Body of `POST /api/emulation/start` and `POST /api/host/hints`: the whole configuration, no ids. */
export interface StartRequest {
  links: StartLink[];
  settings: Settings;
}

/**
 * The accepted start request as `GET /api/emulation` reports it. Links carry extra keys of the controller (assigned id,
 * index, planned lcore) that the frontend ignores.
 */
export interface RunningConfig {
  links: StartLink[];
  settings: Settings;
}

/** The document kept in localStorage (key `dpds.config.v1`). */
export interface LocalConfig {
  version: 1;
  links: Link[];
  settings: Settings;
}

export interface ErrorBody {
  message: string;
}
