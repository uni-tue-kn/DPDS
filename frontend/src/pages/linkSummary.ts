// Read-only text views of a link's config: the cells of the Settings table and the derived limit line of the form.
import { RATE_UNITS, SIZE_UNITS, TIME_UNITS, formatUnit } from '../common/units';
import {
  GBIT,
  MBIT,
  MS,
  bufferCapacity,
  delayerToDto,
  frameToDto,
  histogramToDto,
  limiterToDto,
  lossToDto,
} from './linkForm';
import type { ConfigForm } from './linkForm';
import type { DelayerDto, LimiterDto, LossModelDto } from '../api/types';

const time = (ns: number) => formatUnit(ns, TIME_UNITS, MS);
const rate = (bps: number) => formatUnit(bps, RATE_UNITS, GBIT);
const size = (bit: number) => formatUnit(bit, SIZE_UNITS, MBIT);

/** "Normal" for "NormalDistribution". */
const distributionName = (d: DelayerDto['distribution']) => d.replace(/Distribution$/, '');

/** "Normal 10 ms / 1 ms"; the deterministic distribution has no std to show; "none" without a delayer. */
function delayText(d: DelayerDto | null | undefined): string {
  if (!d) return 'none';
  const name = distributionName(d.distribution);
  return d.distribution === 'DeterministicDistribution'
    ? `${name} ${time(d.mean_ns)}`
    : `${name} ${time(d.mean_ns)} / ${time(d.std_ns)}`;
}

/** "spacing 10 Gbit/s", "policing 10 Gbit/s" or "none". */
function limiterText(l: LimiterDto | null | undefined): string {
  if (!l) return 'none';
  return `${l.mode === 'Spacer' ? 'spacing' : 'policing'} ${rate(l.rate_bps)}`;
}

/** "Random p=0.01", "Gilbert-Elliott p=0.01, r=0.2, h=0.3, k=0.9" or "none". */
function lossText(l: LossModelDto | null | undefined): string {
  if (!l) return 'none';
  return l.type === 'Random' ? `Random p=${l.p}` : `Gilbert-Elliott p=${l.p}, r=${l.r}, h=${l.h}, k=${l.k}`;
}

/** "auto", "off" (delay statistics off) or e.g. "64 bins, 1 µs – 50 µs" ("auto" for an edge that is empty). */
function histogramText(f: ConfigForm): string {
  if (!f.delay_stats) return 'off';
  const h = histogramToDto('Histogram', f.hist_bins, f.hist_lo, f.hist_hi);
  if (!h) return 'auto';
  const range =
    h.lo_ns == null && h.hi_ns == null
      ? null
      : `${h.lo_ns == null ? 'auto' : time(h.lo_ns)} \u2013 ${h.hi_ns == null ? 'auto' : time(h.hi_ns)}`;
  return [h.bins == null ? null : `${h.bins} bins`, range].filter(Boolean).join(', ');
}

export interface RowSummary {
  buffer: string | null;
  delay: string | null;
  limiter: string | null;
  loss: string | null;
  histogram: string | null;
}

/** One text per table cell of a form; null while the values of that cell are not valid yet (shown as "invalid"). */
export function rowSummary(f: ConfigForm): RowSummary {
  const attempt = (make: () => string): string | null => {
    try {
      return make();
    } catch {
      return null;
    }
  };
  return {
    buffer: attempt(() => `${bufferCapacity(f.buffer_capacity)} packets`),
    delay: attempt(() => delayText(f.delayer_added ? delayerToDto(f, frameToDto(f)) : null)),
    limiter: attempt(() => limiterText(f.limiter_added ? limiterToDto(f, frameToDto(f)) : null)),
    loss: attempt(() => lossText(f.dropper_added ? lossToDto(f) : null)),
    histogram: attempt(() => histogramText(f)),
  };
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
