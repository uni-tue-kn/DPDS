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

/** Index of each field's default unit (ms, Mpps, Gbit/s) and of Mbit for the derived limit: what a new form and a zero or empty value show. */
export const MS = 2;
export const MPPS = 2;
export const GBIT = 3;
export const MBIT = 2;
/** Default unit of the buffer capacity: packets (a zero or empty value shows it). */
export const PACKETS = 0;

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

/** The form after picking a delay accuracy option; Reordering needs the token bucket, so it replaces the Spacer. */
export function withAccuracy(f: ConfigForm, a: Accuracy): ConfigForm {
  return {
    ...f,
    reordering: a === 'Reordering',
    activate_correlation: a === 'Correlation',
    limiter_mode: a === 'Reordering' ? 'TokenBucketFilter' : f.limiter_mode,
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
    hist_bins: '',
    hist_lo: emptyUnit(MS),
    hist_hi: emptyUnit(MS),
    iat_bins: '',
    iat_lo: emptyUnit(MS),
    iat_hi: emptyUnit(MS),
    delay_stats: true,
  };
}

const str = (v: number | null | undefined) => (v == null ? '' : String(v));
const optUnit = (v: number | null | undefined, units: Unit[], dflt: number) =>
  v == null ? emptyUnit(dflt) : fromBase(v, units, dflt);

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
    hist_bins: str(c.histogram?.bins),
    hist_lo: optUnit(c.histogram?.lo_ns, TIME_UNITS, MS),
    hist_hi: optUnit(c.histogram?.hi_ns, TIME_UNITS, MS),
    iat_bins: str(c.iat_histogram?.bins),
    iat_lo: optUnit(c.iat_histogram?.lo_ns, TIME_UNITS, MS),
    iat_hi: optUnit(c.iat_histogram?.hi_ns, TIME_UNITS, MS),
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
export function frameToDto(f: ConfigForm): number | null {
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

export function delayerToDto(f: ConfigForm, frame: number | null): DelayerDto {
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

export function lossToDto(f: ConfigForm): LossModelDto {
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

export function limiterToDto(f: ConfigForm, frame: number | null) {
  if (!LIMITER_MODES.includes(f.limiter_mode)) throw new Error(`Unknown limiter mode "${f.limiter_mode}"`);
  return { mode: f.limiter_mode, rate_bps: limiterRate(f.rate_bps, frame) };
}

/** The buffer capacity as an integer packet count; a number of packets that is not a non-negative whole one is refused. */
export function bufferCapacity(v: UnitValue): number {
  const label = 'Buffer capacity';
  const n = toBase(v, PACKET_UNITS);
  if (n === null) throw new Error(`${label} must be a number`);
  if (!Number.isInteger(n)) throw new Error(`${label} must be a whole number of packets`);
  if (n < 0) throw new Error(`${label} must not be negative`);
  return n;
}

/** Reordering excludes correlation and the Spacer (the dpds buffer refuses both); only with the Delayer stage does it count. */
function checkAccuracy(f: ConfigForm) {
  if (!reorderingActive(f)) return;
  if (f.activate_correlation) throw new Error('Reordering cannot be combined with correlation');
  if (f.limiter_added && f.limiter_mode === 'Spacer') {
    throw new Error('Reordering works only with the TokenBucketFilter limiter, not Spacer');
  }
}

/** An optional histogram: null while all three fields are empty; `name` prefixes the messages ("Histogram", "IAT histogram"). */
export function histogramToDto(name: string, bins: string, lo: UnitValue, hi: UnitValue): HistogramDto | null {
  const b = optInt(bins, `${name} bins`);
  const lo_ns = lo.text.trim() === '' ? null : unitNum(lo, TIME_UNITS, `${name} lo`);
  const hi_ns = hi.text.trim() === '' ? null : unitNum(hi, TIME_UNITS, `${name} hi`);
  return b == null && lo_ns == null && hi_ns == null ? null : { bins: b, lo_ns, hi_ns };
}

function configToDto(f: ConfigForm, frame: number | null): ConfigDto {
  const iat_histogram = histogramToDto('IAT histogram', f.iat_bins, f.iat_lo, f.iat_hi);
  return {
    buffer_capacity: bufferCapacity(f.buffer_capacity),
    reordering: reorderingActive(f),
    delayer: f.delayer_added ? delayerToDto(f, frame) : null,
    limiter: f.limiter_added ? limiterToDto(f, frame) : null,
    loss_model: f.dropper_added ? lossToDto(f) : null,
    seed: seed(f.seed),
    histogram: histogramToDto('Histogram', f.hist_bins, f.hist_lo, f.hist_hi),
    ...(iat_histogram && { iat_histogram }),
    delay_stats: f.delay_stats,
  };
}

/** What a link stores beside its name: the config and the UI-only frame size (left out when empty). */
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
    histogramToDto('Histogram', f.hist_bins, f.hist_lo, f.hist_hi);
    histogramToDto('IAT histogram', f.iat_bins, f.iat_lo, f.iat_hi);
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

/** The controller's link-name rule (`valid_name` in `api.rs`). */
export const LINK_NAME = /^[A-Za-z0-9_.-]{1,32}$/;
export const LINK_NAME_RULE = 'Name must be 1 to 32 characters: letters A-Z a-z, digits, underscore, dot or dash';

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * What is wrong with a stored or imported link, or null when the form would accept it unchanged: the name rule,
 * non-empty ports, an optional frame size, and a config that survives the form's own round trip
 * (`configToForm` then the conversion of `formToDto`, minus the delay accuracy rule).
 */
export function linkProblem(v: unknown): string | null {
  try {
    if (!isObject(v)) throw new Error('Link is not an object');
    if (typeof v.id !== 'number' || !Number.isInteger(v.id) || v.id < 1) {
      throw new Error('Link id must be a positive whole number');
    }
    if (typeof v.name !== 'string' || !LINK_NAME.test(v.name)) throw new Error(LINK_NAME_RULE);
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
