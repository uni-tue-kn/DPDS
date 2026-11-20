// Form model of the Link settings and Histogram modals: every number is kept as the typed string so inputs can be emptied and
// retyped; `formToDto` validates and converts back to the wire type.
import { errorMessage } from '../common/format';
import type { ConfigDto, DelayerDto, HistogramDto, Distribution, LimiterMode, LossModelDto } from '../api/types';
import {
  PACKET_UNITS,
  PPS_UNITS,
  RATE_UNITS,
  TIME_UNITS,
  convertBase,
  emptyUnit,
  fromBase,
  parseFrame,
  shiftDecimal,
  sourceOf,
  sameFamily,
  toBase,
} from '../common/units';
import type { Unit, UnitValue } from '../common/units';

export const DISTRIBUTIONS: Distribution[] = [
  'DeterministicDistribution',
  'LogNormalDistribution',
  'NormalDistribution',
  'GammaDistribution',
  'UniformDistribution',
];
export const LIMITER_MODES: LimiterMode[] = ['Spacer', 'TokenBucketFilter'];
/** The UI name of each limiter mode; the API keeps `TokenBucketFilter`. */
export const LIMITER_MODE_LABELS: Record<LimiterMode, string> = { Spacer: 'Spacer', TokenBucketFilter: 'Policer' };

/** Index of each field's default unit (ms, Mpps, Gbit/s) and of Mbit for the derived limit: what a new form and a zero or empty value show. */
export const MS = 2;
export const MPPS = 2;
export const GBIT = 3;
export const MBIT = 2;
/** Default unit of the buffer capacity: packets (a zero or empty value shows it). */
export const PACKETS = 0;

/** The histogram defaults of P4TG: an empty field is its default (bins, lo and hi in ns). */
export const HIST_BINS = 16;
export const HIST_LO_NS = 1024;
export const HIST_HI_NS = 2048;
export const IAT_BINS = 16;
export const IAT_LO_NS = 0;
export const IAT_HI_NS = 1024;

/**
 * The units of the two rate fields once a frame size is known: own family first, so an index into the own family means the
 * same unit with and without the other one.
 */
export const INIT_RATE_UNITS = [...PPS_UNITS, ...RATE_UNITS];
export const LIMITER_RATE_UNITS = [...RATE_UNITS, ...PPS_UNITS];

export interface ConfigForm {
  /** A stage that is not added is not sent; its typed values stay in the form while it is removed. */
  delayer_added: boolean;
  limiter_added: boolean;
  dropper_added: boolean;
  distribution: Distribution;
  mean_ns: UnitValue;
  std_ns: UnitValue;
  activate_correlation: boolean;
  half_life_period_ns: UnitValue;
  initialization_rate_pps: UnitValue;
  limiter_mode: LimiterMode;
  rate_bps: UnitValue;
  loss_type: LossModelDto['type'];
  random_p: string;
  ge_p: string;
  ge_r: string;
  ge_k: string;
  ge_h: string;
  buffer_capacity: UnitValue;
  /** With `activate_correlation`, the "Delay accuracy" mode of the Delayer: Basic (neither), Reordering or Correlation. */
  reordering: boolean;
  seed: string;
  /** UI only (bytes, typed): not part of the config, kept beside it in the link. */
  avg_frame_size: string;
  hist_bins: string;
  hist_lo: UnitValue;
  hist_hi: UnitValue;
  iat_bins: string;
  iat_lo: UnitValue;
  iat_hi: UnitValue;
  delay_stats: boolean;
}

/** Conditionally required fields: the form marks them and refuses them empty only while the rule holds. */
export const stdRequired = (f: ConfigForm) => f.distribution !== 'DeterministicDistribution';
export const halfLifeRequired = (f: ConfigForm) => f.activate_correlation;

export type Accuracy = 'Basic' | 'Reordering' | 'Correlation';
export const ACCURACIES: Accuracy[] = ['Basic', 'Reordering', 'Correlation'];
/** The delay accuracy option the two booleans show: reordering wins over correlation. */
export const accuracyOf = (f: ConfigForm): Accuracy =>
  f.reordering ? 'Reordering' : f.activate_correlation ? 'Correlation' : 'Basic';
/** Without the Delayer stage the config is sent with Basic, so reordering counts only with it. */
export const reorderingActive = (f: ConfigForm) => f.delayer_added && f.reordering;

/** The form after picking a delay accuracy option. */
export function withAccuracy(f: ConfigForm, a: Accuracy): ConfigForm {
  return {
    ...f,
    reordering: a === 'Reordering',
    activate_correlation: a === 'Correlation',
  };
}

/** True while the rate field of an added stage shows a unit of the other family: only a frame size can convert it. */
const crossUnit = (added: boolean, v: UnitValue, units: Unit[]) => added && !sameFamily(units[v.unit], units[0]);
export const frameRequired = (f: ConfigForm) =>
  crossUnit(f.delayer_added, f.initialization_rate_pps, INIT_RATE_UNITS) ||
  crossUnit(f.limiter_added, f.rate_bps, LIMITER_RATE_UNITS);

export function defaultConfigForm(): ConfigForm {
  return {
    delayer_added: false,
    limiter_added: false,
    dropper_added: false,
    distribution: 'NormalDistribution',
    mean_ns: fromBase(1e6, TIME_UNITS, MS),
    std_ns: fromBase(1e5, TIME_UNITS, MS),
    activate_correlation: false,
    half_life_period_ns: fromBase(0, TIME_UNITS, MS),
    initialization_rate_pps: emptyUnit(MPPS),
    limiter_mode: 'Spacer',
    rate_bps: fromBase(1e10, RATE_UNITS, GBIT),
    loss_type: 'Random',
    random_p: '0',
    ge_p: '0.01',
    ge_r: '0.2',
    ge_k: '1',
    ge_h: '0',
    buffer_capacity: fromBase(100000, PACKET_UNITS, PACKETS),
    reordering: false,
    seed: '',
    avg_frame_size: '',
    hist_bins: String(HIST_BINS),
    hist_lo: fromBase(HIST_LO_NS, TIME_UNITS, MS),
    hist_hi: fromBase(HIST_HI_NS, TIME_UNITS, MS),
    iat_bins: String(IAT_BINS),
    iat_lo: fromBase(IAT_LO_NS, TIME_UNITS, MS),
    iat_hi: fromBase(IAT_HI_NS, TIME_UNITS, MS),
    delay_stats: true,
  };
}

const str = (v: number | null | undefined) => (v == null ? '' : String(v));
const bins = (v: number | null | undefined, dflt: number) => String(v ?? dflt);
const timeNs = (v: number | null | undefined, dflt: number) => fromBase(v ?? dflt, TIME_UNITS, MS);

/** `frame`: the stored average frame size of the link that holds `c` (null or absent: none). */
export function configToForm(c: ConfigDto, frame?: number | null): ConfigForm {
  const f: ConfigForm = {
    ...defaultConfigForm(),
    delayer_added: c.delayer != null,
    limiter_added: c.limiter != null,
    dropper_added: c.loss_model != null,
    buffer_capacity: fromBase(c.buffer_capacity, PACKET_UNITS, PACKETS),
    reordering: c.reordering,
    seed: str(c.seed),
    avg_frame_size: str(frame),
    hist_bins: bins(c.histogram?.bins, HIST_BINS),
    hist_lo: timeNs(c.histogram?.lo_ns, HIST_LO_NS),
    hist_hi: timeNs(c.histogram?.hi_ns, HIST_HI_NS),
    iat_bins: bins(c.iat_histogram?.bins, IAT_BINS),
    iat_lo: timeNs(c.iat_histogram?.lo_ns, IAT_LO_NS),
    iat_hi: timeNs(c.iat_histogram?.hi_ns, IAT_HI_NS),
    delay_stats: c.delay_stats ?? true,
  };
  const d = c.delayer;
  if (d) {
    f.distribution = d.distribution;
    f.mean_ns = fromBase(d.mean_ns, TIME_UNITS, MS);
    f.std_ns = fromBase(d.std_ns, TIME_UNITS, MS);
    f.activate_correlation = d.activate_correlation;
    f.half_life_period_ns = fromBase(d.half_life_period_ns, TIME_UNITS, MS);
    f.initialization_rate_pps = fromBase(d.initialization_rate_pps, PPS_UNITS, MPPS);
  }
  if (c.limiter) {
    f.limiter_mode = c.limiter.mode;
    f.rate_bps = fromBase(c.limiter.rate_bps, RATE_UNITS, GBIT);
  }
  const l = c.loss_model;
  if (l) {
    f.loss_type = l.type;
    if (l.type === 'Random') {
      f.random_p = String(l.p);
    } else {
      f.ge_p = String(l.p);
      f.ge_r = String(l.r);
      f.ge_k = String(l.k);
      f.ge_h = String(l.h);
    }
  }
  return f;
}

/** A loss probability: a number within [0, 1]. */
function prob(s: string, label: string): number {
  const n = num(s, label);
  if (n < 0 || n > 1) throw new Error(`${label} must be between 0 and 1`);
  return n;
}

function num(s: string, label: string): number {
  const n = Number(s);
  if (s.trim() === '' || !Number.isFinite(n)) throw new Error(`${label} must be a number`);
  return n;
}

/**
 * A unit field in its base unit. Empty is 0 while the field is not required, an error while it is. A
 * negative value is refused while the field is in use (required); the message names the field only.
 */
function unitNum(v: UnitValue, units: Unit[], label: string, required = true): number {
  if (v.text.trim() === '' && !required) return 0;
  const n = toBase(v, units);
  if (n === null) throw new Error(`${label} must be a number`);
  if (required && n < 0) throw new Error(`${label} must not be negative`);
  return n;
}

/**
 * A rate field in the base unit of its own family (units[0]). A value typed in the other family is converted with the
 * frame size and rounded to a whole base unit.
 */
function rateBase(
  v: UnitValue,
  units: Unit[],
  label: string,
  frame: number | null,
  checkExact: (exact: number) => void = () => {},
): number {
  const src = sourceOf(v);
  const n = toBase(src, units);
  if (n === null) throw new Error(`${label} must be a number`);
  const from = units[src.unit];
  if (sameFamily(from, units[0])) {
    checkExact(n);
    return n;
  }
  if (frame === null) throw new Error(`Average frame size is required to convert ${label} from ${from.label}`);
  const exact = convertBase(n, from, units[0], frame);
  checkExact(exact);
  return Math.round(exact);
}

/** No default exists for this rate: the user always chooses it. */
function initRate(v: UnitValue, frame: number | null): number {
  const label = 'Initialization rate';
  if (v.text.trim() === '') throw new Error(`${label} is required`);
  return rateBase(v, INIT_RATE_UNITS, label, frame, (exact) => {
    if (exact < 1) throw new Error(`${label} must be at least 1 pps`);
  });
}

/** The limiter rate: empty, negative or 0 (also after rounding to a whole bit/s) is refused, the controller needs > 0. */
function limiterRate(v: UnitValue, frame: number | null): number {
  const label = 'Rate';
  if (v.text.trim() === '') throw new Error(`${label} must be a number`);
  const n = rateBase(v, LIMITER_RATE_UNITS, label, frame);
  if (n < 0) throw new Error(`${label} must not be negative`);
  if (n === 0) throw new Error(`${label} must be greater than 0`);
  return n;
}

/** The average frame size in bytes, null when empty; an invalid value is refused even if no rate field needs it. */
function frameToDto(f: ConfigForm): number | null {
  if (f.avg_frame_size.trim() === '') return null;
  const n = parseFrame(f.avg_frame_size);
  if (n === null) throw new Error('Average frame size must be a number of at least 64 (bytes)');
  return n;
}

function int(s: string, label: string): number {
  const n = num(s, label);
  if (!Number.isInteger(n)) throw new Error(`${label} must be an integer`);
  return n;
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
const NETDEV = /^[A-Za-z0-9_.-]{1,15}$/;

/**
 * The controller's port rule without knowing its mode: a PCI address, a `net_null<N>` / `net_ring<N>` vdev, or a kernel
 * interface name (virtual mode). The controller enforces the mode of the installation.
 */
export function portError(port: string): string | null {
  const netdev = NETDEV.test(port) && port !== 'lo' && port !== '.' && port !== '..' && !port.startsWith('net_');
  return PCI.test(port) || VDEV.test(port) || netdev
    ? null
    : `"${port}" is neither a PCI address (0000:65:00.0), a vdev (net_null0, net_ring0) nor an interface name (eth1)`;
}

function delayerToDto(f: ConfigForm, frame: number | null): DelayerDto {
  if (!DISTRIBUTIONS.includes(f.distribution)) throw new Error(`Delayer distribution "${f.distribution}" is unknown`);
  return {
    distribution: f.distribution,
    mean_ns: unitNum(f.mean_ns, TIME_UNITS, 'Mean delay'),
    std_ns: unitNum(f.std_ns, TIME_UNITS, 'Delay std', stdRequired(f)),
    activate_correlation: f.activate_correlation,
    half_life_period_ns: unitNum(f.half_life_period_ns, TIME_UNITS, 'Half-life period', halfLifeRequired(f)),
    initialization_rate_pps: initRate(f.initialization_rate_pps, frame),
  };
}

function lossToDto(f: ConfigForm): LossModelDto {
  return f.loss_type === 'Random'
    ? { type: 'Random', p: prob(f.random_p, 'Random loss probability') }
    : {
        type: 'GilbertElliot',
        p: prob(f.ge_p, 'Gilbert-Elliot p'),
        r: prob(f.ge_r, 'Gilbert-Elliot r'),
        k: prob(f.ge_k, 'Gilbert-Elliot k'),
        h: prob(f.ge_h, 'Gilbert-Elliot h'),
      };
}

function limiterToDto(f: ConfigForm, frame: number | null) {
  if (!LIMITER_MODES.includes(f.limiter_mode)) throw new Error(`Unknown limiter mode "${f.limiter_mode}"`);
  return { mode: f.limiter_mode, rate_bps: limiterRate(f.rate_bps, frame) };
}

/** The buffer capacity as an integer packet count; a number of packets that is not a non-negative whole one is refused. */
function bufferCapacity(v: UnitValue): number {
  const label = 'Buffer capacity';
  const n = toBase(v, PACKET_UNITS);
  if (n === null) throw new Error(`${label} must be a number`);
  if (!Number.isInteger(n)) throw new Error(`${label} must be a whole number of packets`);
  if (n < 0) throw new Error(`${label} must not be negative`);
  return n;
}

/** Reordering excludes correlation (the dpds buffer refuses it); only with the Delayer stage does it count. */
function checkAccuracy(f: ConfigForm) {
  if (!reorderingActive(f)) return;
  if (f.activate_correlation) throw new Error('Reordering cannot be combined with correlation');
}

const binsOrDefault = (s: string, dflt: number) => (s.trim() === '' ? String(dflt) : s);
const timeOrDefault = (v: UnitValue, dflt: number) => (v.text.trim() === '' ? fromBase(dflt, TIME_UNITS, MS) : v);

/** The form with every empty histogram field replaced by its default. */
export const withHistogramDefaults = (f: ConfigForm): ConfigForm => ({
  ...f,
  hist_bins: binsOrDefault(f.hist_bins, HIST_BINS),
  hist_lo: timeOrDefault(f.hist_lo, HIST_LO_NS),
  hist_hi: timeOrDefault(f.hist_hi, HIST_HI_NS),
  iat_bins: binsOrDefault(f.iat_bins, IAT_BINS),
  iat_lo: timeOrDefault(f.iat_lo, IAT_LO_NS),
  iat_hi: timeOrDefault(f.iat_hi, IAT_HI_NS),
});

/** A histogram with explicit values: an empty field is its default; `name` prefixes the messages ("Histogram", "IAT histogram"). */
function histogramToDto(
  name: string,
  b: string,
  lo: UnitValue,
  hi: UnitValue,
  d: [number, number, number],
): HistogramDto {
  return {
    bins: int(binsOrDefault(b, d[0]), `${name} bins`),
    lo_ns: unitNum(timeOrDefault(lo, d[1]), TIME_UNITS, `${name} lo`),
    hi_ns: unitNum(timeOrDefault(hi, d[2]), TIME_UNITS, `${name} hi`),
  };
}

const delayHistogram = (f: ConfigForm) =>
  histogramToDto('Histogram', f.hist_bins, f.hist_lo, f.hist_hi, [HIST_BINS, HIST_LO_NS, HIST_HI_NS]);
const iatHistogram = (f: ConfigForm) =>
  histogramToDto('IAT histogram', f.iat_bins, f.iat_lo, f.iat_hi, [IAT_BINS, IAT_LO_NS, IAT_HI_NS]);

function configToDto(f: ConfigForm, frame: number | null): ConfigDto {
  return {
    buffer_capacity: bufferCapacity(f.buffer_capacity),
    reordering: reorderingActive(f),
    delayer: f.delayer_added ? delayerToDto(f, frame) : null,
    limiter: f.limiter_added ? limiterToDto(f, frame) : null,
    loss_model: f.dropper_added ? lossToDto(f) : null,
    seed: seed(f.seed),
    histogram: delayHistogram(f),
    iat_histogram: iatHistogram(f),
    delay_stats: f.delay_stats,
  };
}

/** What a link stores beside its ports: the config and the UI-only frame size (left out when empty). */
export interface FormDto {
  config: ConfigDto;
  avg_frame_size_byte?: number;
}

/**
 * Throws an Error naming the offending field. Stages that are not added are sent as null and not validated; an invalid
 * frame size is refused even when no rate field needs it.
 */
export function formToDto(f: ConfigForm): FormDto {
  checkAccuracy(f);
  return convert(f);
}

const problemOf = (check: () => void): string | null => {
  try {
    check();
    return null;
  } catch (e) {
    return errorMessage(e);
  }
};

/** The first problem of the Buffer, Delayer, Limiter and Dropper fields (the checks of the Link settings modal), or null. */
export const stageProblem = (f: ConfigForm): string | null =>
  problemOf(() => {
    checkAccuracy(f);
    const frame = frameToDto(f);
    bufferCapacity(f.buffer_capacity);
    seed(f.seed);
    if (f.delayer_added) delayerToDto(f, frame);
    if (f.limiter_added) limiterToDto(f, frame);
    if (f.dropper_added) lossToDto(f);
  });

/** The first problem of the delay and IAT histogram fields (the checks of the Histogram modal), or null. */
export const histogramProblem = (f: ConfigForm): string | null =>
  problemOf(() => {
    delayHistogram(f);
    iatHistogram(f);
  });

/** `formToDto` without the delay accuracy rule, so a stored link that breaks it stays listed and openable to be fixed. */
function convert(f: ConfigForm): FormDto {
  const frame = frameToDto(f);
  const config = configToDto(f, frame);
  return frame === null ? { config } : { config, avg_frame_size_byte: frame };
}

/** A bin count that is a power of two: 1, 2, 4, ... */
export const isPowerOfTwo = (n: number) => Number.isSafeInteger(n) && n > 0 && Number.isInteger(Math.log2(n));

/** 2^k for a whole k, negative ones included (a bin narrower than 1 ns). */
const isBinaryWidth = (n: number) => Number.isFinite(n) && n > 0 && Number.isInteger(Math.log2(n));

/** `ns` as the text of the time unit `unit`, exact ("0.064"), or null when it cannot be shown. */
const inUnit = (ns: number, unit: number) => shiftDecimal(String(ns), -TIME_UNITS[unit].exp);

/** Advice on the histogram range: advisory only, nothing here refuses a value. */
export function rangeAdvice(
  value: ConfigForm,
): { aligned: boolean; width: string; suggested: string; exponent: number } | null {
  const hi = value.hist_hi.text.trim() === '' ? null : toBase(value.hist_hi, TIME_UNITS);
  if (hi === null || hi <= 0) return null;
  const lo = value.hist_lo.text.trim() === '' ? null : toBase(value.hist_lo, TIME_UNITS);
  const bins = value.hist_bins.trim() === '' ? NaN : Number(value.hist_bins);
  const width = lo === null ? NaN : (hi - lo) / bins;
  if (lo !== null && Number.isSafeInteger(lo) && isBinaryWidth(width) && lo % width === 0) {
    const shown = inUnit(width, value.hist_hi.unit);
    if (shown !== null) return { aligned: true, width: shown, suggested: '', exponent: Math.log2(width) };
  }
  const next = 2 ** Math.ceil(Math.log2(Math.ceil(hi)));
  const suggested = inUnit(next, value.hist_hi.unit);
  if (!Number.isSafeInteger(next) || suggested === null) return null;
  // Applying the suggestion would change nothing.
  if (lo === 0 && hi === next) return null;
  return { aligned: false, width: '', suggested, exponent: Math.log2(next) };
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * What is wrong with a stored or imported link, or null when the form would accept it unchanged: non-empty ports, an optional frame size, and a config that survives the form's own round trip
 * (`configToForm` then the conversion of `formToDto`, minus the delay accuracy rule).
 */
export function linkProblem(v: unknown): string | null {
  try {
    if (!isObject(v)) throw new Error('Link is not an object');
    if (typeof v.id !== 'number' || !Number.isInteger(v.id) || v.id < 1) {
      throw new Error('Link id must be a positive whole number');
    }
    if (typeof v.ingress !== 'string' || v.ingress === '') throw new Error('Ingress port is required');
    if (typeof v.egress !== 'string' || v.egress === '') throw new Error('Egress port is required');
    const frame = v.avg_frame_size_byte ?? null;
    if (frame !== null && typeof frame !== 'number') throw new Error('Average frame size must be a number');
    if (!isObject(v.config)) throw new Error('Link config is missing');
    convert(configToForm(v.config as unknown as ConfigDto, frame));
    return null;
  } catch (e) {
    return errorMessage(e);
  }
}
