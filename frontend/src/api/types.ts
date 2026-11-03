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
  activate: boolean;
  rate_bps: number;
  limit_bit: number;
}

export type LossModelDto =
  | { type: 'Random'; p: number }
  | { type: 'GilbertElliot'; p: number; r: number; k: number; h: number; state_is_good?: boolean };

/** All three optional (null/absent = automatic default: 256 bins, mean +- 6 std). */
export interface HistogramDto {
  bins?: number | null;
  lo_ns?: number | null;
  hi_ns?: number | null;
}

export interface ConfigDto {
  buffer_capacity: number;
  reordering: boolean;
  delayer: DelayerDto;
  limiter: LimiterDto;
  loss_model: LossModelDto;
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
}

export type LinkInput = Omit<Link, 'id'>;

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
  /** The netdev (or a master/upper device over it) carries a kernel route: management guard. */
  kernel_route: boolean;
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
  mlx5_devargs: string;
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
  /** Links or settings changed since the emulation started. */
  pending_restart: boolean;
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

export interface Preset {
  id: number;
  name: string;
  description?: string | null;
  config: ConfigDto;
}

export type PresetInput = Omit<Preset, 'id'>;

/** `GET /api/export`: no ids, no runtime state. */
export interface ExportDocument {
  format: 'dpds-controller-export';
  version: 2;
  settings: Settings;
  links: LinkInput[];
  presets: PresetInput[];
}

export interface ImportSummary {
  links: number;
  presets: number;
}

export interface ErrorBody {
  message: string;
}
