// Form model of the link editor: every number is kept as the typed string so inputs can be emptied and
// retyped; `configToDto` validates and converts back to the wire type.
import type { ConfigDto, Distribution, LimiterMode, LossModelDto } from '../api/types';

export const DISTRIBUTIONS: Distribution[] = [
  'DeterministicDistribution', 'LogNormalDistribution', 'NormalDistribution', 'GammaDistribution', 'UniformDistribution',
];
export const LIMITER_MODES: LimiterMode[] = ['Spacer', 'TokenBucketFilter'];

export interface ConfigForm {
  distribution: Distribution;
  mean_ns: string;
  std_ns: string;
  activate_correlation: boolean;
  half_life_period_ns: string;
  limiter_activate: boolean;
  limiter_mode: LimiterMode;
  rate_bps: string;
  limit_bit: string;
  loss_type: LossModelDto['type'];
  random_p: string;
  ge_p: string;
  ge_r: string;
  ge_k: string;
  ge_h: string;
  ge_good: boolean;
  buffer_capacity: string;
  reordering: boolean;
  seed: string;
  hist_bins: string;
  hist_lo: string;
  hist_hi: string;
  delay_stats: boolean;
}

export function defaultConfigForm(): ConfigForm {
  return {
    distribution: 'NormalDistribution', mean_ns: '1000000', std_ns: '100000',
    activate_correlation: false, half_life_period_ns: '0',
    limiter_activate: false, limiter_mode: 'Spacer', rate_bps: '10000000000', limit_bit: '12000000',
    loss_type: 'Random', random_p: '0', ge_p: '0.01', ge_r: '0.2', ge_k: '1', ge_h: '0', ge_good: true,
    buffer_capacity: '100000', reordering: false, seed: '', hist_bins: '', hist_lo: '', hist_hi: '',
    delay_stats: true,
  };
}

const str = (v: number | null | undefined) => (v == null ? '' : String(v));

export function configToForm(c: ConfigDto): ConfigForm {
  const f: ConfigForm = {
    ...defaultConfigForm(),
    distribution: c.delayer.distribution,
    mean_ns: String(c.delayer.mean_ns),
    std_ns: String(c.delayer.std_ns),
    activate_correlation: c.delayer.activate_correlation,
    half_life_period_ns: String(c.delayer.half_life_period_ns),
    limiter_activate: c.limiter.activate,
    limiter_mode: c.limiter.mode,
    rate_bps: String(c.limiter.rate_bps),
    limit_bit: String(c.limiter.limit_bit),
    loss_type: c.loss_model.type,
    buffer_capacity: String(c.buffer_capacity),
    reordering: c.reordering,
    seed: str(c.seed),
    hist_bins: str(c.histogram?.bins),
    hist_lo: str(c.histogram?.lo_ns),
    hist_hi: str(c.histogram?.hi_ns),
    delay_stats: c.delay_stats ?? true,
  };
  if (c.loss_model.type === 'Random') {
    f.random_p = String(c.loss_model.p);
  } else {
    f.ge_p = String(c.loss_model.p);
    f.ge_r = String(c.loss_model.r);
    f.ge_k = String(c.loss_model.k);
    f.ge_h = String(c.loss_model.h);
    f.ge_good = c.loss_model.state_is_good ?? true;
  }
  return f;
}

function num(s: string, label: string): number {
  const n = Number(s);
  if (s.trim() === '' || !Number.isFinite(n)) throw new Error(`${label} must be a number`);
  return n;
}

function int(s: string, label: string): number {
  const n = num(s, label);
  if (!Number.isInteger(n)) throw new Error(`${label} must be an integer`);
  return n;
}

function optInt(s: string, label: string): number | null {
  return s.trim() === '' ? null : int(s, label);
}

/** A seed is a u64 on the wire but a JSON number here: only integers up to 2^53-1 survive the round trip. */
function seed(s: string): number | null {
  const t = s.trim();
  if (t === '') return null;
  const n = Number(t);
  if (!/^\d+$/.test(t) || !Number.isSafeInteger(n)) {
    throw new Error(`Seed must be a whole number between 0 and ${Number.MAX_SAFE_INTEGER}`);
  }
  return n;
}

const PCI = /^[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]$/;
const VDEV = /^net_(null|ring)\d+$/;

/** The controller's port rule: a PCI address or a `net_null<N>` / `net_ring<N>` vdev, nothing else. */
export function portError(port: string): string | null {
  return PCI.test(port) || VDEV.test(port)
    ? null
    : `"${port}" is neither a PCI address (0000:65:00.0) nor a vdev (net_null0, net_ring0)`;
}

/** Throws an Error naming the offending field. */
export function configToDto(f: ConfigForm): ConfigDto {
  const loss_model: LossModelDto = f.loss_type === 'Random'
    ? { type: 'Random', p: num(f.random_p, 'Random loss probability') }
    : {
      type: 'GilbertElliot',
      p: num(f.ge_p, 'Gilbert-Elliot p'),
      r: num(f.ge_r, 'Gilbert-Elliot r'),
      k: num(f.ge_k, 'Gilbert-Elliot k'),
      h: num(f.ge_h, 'Gilbert-Elliot h'),
      state_is_good: f.ge_good,
    };
  const bins = optInt(f.hist_bins, 'Histogram bins');
  const lo_ns = f.hist_lo.trim() === '' ? null : num(f.hist_lo, 'Histogram lo (ns)');
  const hi_ns = f.hist_hi.trim() === '' ? null : num(f.hist_hi, 'Histogram hi (ns)');
  return {
    buffer_capacity: int(f.buffer_capacity, 'Buffer capacity'),
    reordering: f.reordering,
    delayer: {
      distribution: f.distribution,
      mean_ns: num(f.mean_ns, 'Mean delay (ns)'),
      std_ns: num(f.std_ns, 'Delay std (ns)'),
      activate_correlation: f.activate_correlation,
      half_life_period_ns: num(f.half_life_period_ns, 'Half-life period (ns)'),
    },
    limiter: {
      mode: f.limiter_mode,
      activate: f.limiter_activate,
      rate_bps: num(f.rate_bps, 'Rate (bit/s)'),
      limit_bit: num(f.limit_bit, 'Burst limit (bit)'),
    },
    loss_model,
    seed: seed(f.seed),
    histogram: bins == null && lo_ns == null && hi_ns == null ? null : { bins, lo_ns, hi_ns },
    delay_stats: f.delay_stats,
  };
}

/** "7" -> 7; empty -> null (automatic). */
export function parseWorker(s: string): number | null {
  const t = s.trim();
  if (t === '') return null;
  const n = Number(t);
  if (!/^\d+$/.test(t) || !Number.isSafeInteger(n)) throw new Error(`Worker lcore: "${t}" is not a core number`);
  return n;
}
