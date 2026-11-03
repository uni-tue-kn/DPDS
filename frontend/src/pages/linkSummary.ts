// Read-only views of a stored config for the Links list (one cell per section) and the inspect panel (every field).
import { PPS_UNITS, RATE_UNITS, SIZE_UNITS, TIME_UNITS, formatUnit } from '../common/units';
import { GBIT, MBIT, MPPS, MS } from './linkForm';
import type { ConfigDto } from '../api/types';

export const time = (ns: number) => formatUnit(ns, TIME_UNITS, MS);
export const rate = (bps: number) => formatUnit(bps, RATE_UNITS, GBIT);
const size = (bit: number) => formatUnit(bit, SIZE_UNITS, MBIT);

export const pps = (v: number) => formatUnit(v, PPS_UNITS, MPPS);

/** "Normal" for "NormalDistribution". */
export const distributionName = (d: ConfigDto['delayer']['distribution']) => d.replace(/Distribution$/, '');

/** "Normal 10 ms / 1 ms"; the deterministic distribution has no std to show. */
export function delayCell(c: ConfigDto): string {
  const d = c.delayer;
  const name = distributionName(d.distribution);
  return d.distribution === 'DeterministicDistribution'
    ? `${name} ${time(d.mean_ns)}`
    : `${name} ${time(d.mean_ns)} / ${time(d.std_ns)}`;
}

export const reorderingCell = (c: ConfigDto) => (c.reordering ? '+' : '\u2212');

/** "spacing 10 Gbit/s", "policing 10 Gbit/s" or "none". */
export function limiterCell(c: ConfigDto): string {
  if (!c.limiter.activate) return 'none';
  return `${c.limiter.mode === 'Spacer' ? 'spacing' : 'policing'} ${rate(c.limiter.rate_bps)}`;
}

/** "Random p=0.01" or "Gilbert-Elliott p=0.01, r=0.2, h=0.3, k=0.9". */
export function lossCell(c: ConfigDto): string {
  const l = c.loss_model;
  return l.type === 'Random' ? `Random p=${l.p}` : `Gilbert-Elliott p=${l.p}, r=${l.r}, h=${l.h}, k=${l.k}`;
}

/** dpds-core's frame size for the derived limiter limit: 1518 B frame + 4 B FEC + 20 B L1 overhead. */
const MAX_FRAME_WIRE_BYTES = 1542;

/** The read-only limiter limit line of the form; the number is left out while the capacity is no valid count. */
export function derivedLimitText(capacity: string): string {
  const text = 'Limit: derived from buffer capacity × 1542 B';
  const n = Number(capacity);
  if (capacity.trim() === '' || !Number.isInteger(n) || n < 0) return text;
  return `${text} = ${size(n * MAX_FRAME_WIRE_BYTES * 8)}`;
}
