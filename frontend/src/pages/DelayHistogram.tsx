import type { ChartData, ChartOptions } from 'chart.js';
import { useEffect, useMemo, useState } from 'react';
import { Bar } from 'react-chartjs-2';
import * as api from '../api/client';
import type { HistogramView } from '../api/types';
import { errorMessage } from '../common/format';
import { startPolling } from '../common/polling';
import { useTheme } from '../common/theme';
import { chartOptions } from '../components/charts';
import ChartBox from './ChartBox';
import HistogramTables from './HistogramTables';

export type HistKind = 'delay' | 'iat';

const POLL_MS = 1000;
/** Width to height of the histograms. */
const HIST_ASPECT = 4;
const BAR_COLOR = 'rgba(231, 76, 60, 0.6)';
const IAT_BAR_COLOR = 'rgba(53, 162, 235, 0.6)';
/** The line colours by percentile index: the RX palette for the delay, the TX palette for the IAT (the departure side). */
const DELAY_PALETTE = ['#e74c3c', '#d35400', '#e67e22', '#af601a'];
const IAT_PALETTE = ['#3c82e7', '#2ecc71', '#16a085', '#1f618d'];
const PERCENTILES: [string, 'p50' | 'p90' | 'p99' | 'p999'][] = [
  ['p50', 'p50'],
  ['p90', 'p90'],
  ['p99', 'p99'],
  ['p99.9', 'p999'],
];

/** Two annotations per percentile with a value: a dashed line at its place on the category axis (bin i is centred on i) and its label. */
export function percentileAnnotations(hist: HistogramView, palette: string[], maxCount: number) {
  const p = hist.percentiles;
  const out: Record<string, unknown> = {};
  if (!p) return out;
  PERCENTILES.forEach(([label, key], i) => {
    const value = p[key];
    if (value == null) return;
    const x = (value - hist.lo_ns) / hist.bin_ns - 0.5;
    const color = palette[i];
    out[key] = { type: 'line', xMin: x, xMax: x, borderColor: color, borderDash: [6, 6], borderWidth: 2 };
    out[`${key}Label`] = {
      type: 'label',
      content: label,
      xValue: x - 0.15,
      yValue: maxCount * 0.95 * (1 - 0.065 * i),
      backgroundColor: `${color}80`,
      color: 'black',
      font: { size: 18, family: 'sans-serif' },
      padding: 4,
      borderRadius: 7,
      position: 'center',
      yAdjust: -10,
    };
  });
  return out;
}

/** The value scaled down by 1000 per step while it reaches 1000, with the unit it ends in. */
function timeUnit(ns: number): [number, string] {
  const units = ['ns', '\u00b5s', 'ms', 's'];
  let i = 0;
  let value = ns;
  while (value >= 1000 && i < units.length - 1) {
    value /= 1000;
    i++;
  }
  return [value, units[i] ?? 'ns'];
}

/** "<start> – <end> <unit>", or with a unit on each value where the two differ. */
function rangeLabel(start: number, end: number): string {
  const [a, unitA] = timeUnit(start);
  const [b, unitB] = timeUnit(end);
  return unitA === unitB
    ? `${a.toFixed(2)} \u2013 ${b.toFixed(2)} ${unitA}`
    : `${a.toFixed(2)} ${unitA} \u2013 ${b.toFixed(2)} ${unitB}`;
}

/** Field-wise equality of two JSON values (a poll reply and the histogram on screen). */
function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  const x = a as Record<string, unknown>;
  const y = b as Record<string, unknown>;
  const keys = Object.keys(x);
  return keys.length === Object.keys(y).length && keys.every((k) => same(x[k], y[k]));
}

/** True when a poll reply carries nothing new, so that the chart need not be drawn again. */
export function sameHistogram(shown: HistogramView | undefined, reply: HistogramView): boolean {
  return shown !== undefined && same(shown, reply);
}

/** Mean and standard deviation (population) from the bin centres weighted by the counts, under/overflow excluded; null without a sample. */
function binStats(hist: HistogramView) {
  const n = hist.bins.reduce((a, b) => a + b, 0);
  if (n === 0) return { mean: null, std: null };
  const centre = (i: number) => hist.lo_ns + (i + 0.5) * hist.bin_ns;
  const mean = hist.bins.reduce((a, c, i) => a + c * centre(i), 0) / n;
  const variance = hist.bins.reduce((a, c, i) => a + c * (centre(i) - mean) ** 2, 0) / n;
  return { mean, std: Math.sqrt(variance) };
}

/**
 * The delay histogram of the whole run of link `dir`: its numbers in tables above the bars. With `kind="iat"` it is the
 * IAT histogram of the link instead (blue bars, mean and σ from the bins, no live stats). The owner keeps the last
 * histogram (`hist`, fed through `onHist`) so that this view, remounted on every radio switch, shows it at once.
 */
export default function DelayHistogram({
  dir,
  active = true,
  kind = 'delay',
  hist,
  onHist,
  meanNs,
  stdNs,
  hidden,
  onToggle,
}: {
  dir: number;
  /** Whether the link's tab is shown: a hidden histogram keeps what it has but does not poll. */
  active?: boolean;
  kind?: HistKind;
  /** The last histogram of this link and kind, if any. */
  hist: HistogramView | undefined;
  onHist: (kind: HistKind, hist: HistogramView) => void;
  meanNs?: number | null;
  stdNs?: number | null;
  /** Whether the percentile lines are hidden; the owner keeps it so that it outlives this view and the tab. */
  hidden: boolean;
  onToggle: () => void;
}) {
  const iat = kind === 'iat';
  const name = iat ? 'IAT' : 'Delay';
  const theme = useTheme();
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!active) return;
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const h = await (kind === 'iat' ? api.getIatHistogram(dir) : api.getDelayHistogram(dir));
          if (!alive) return;
          onHist(kind, h);
          setError(undefined);
        } catch (err) {
          if (!alive) return;
          setError(errorMessage(err));
        }
      },
      POLL_MS,
      true,
    );
    return () => {
      alive = false;
      stop();
    };
  }, [dir, kind, active, onHist]);

  // Bar height is the probability of the bin: its count of everything recorded, under and overflow included.
  const bars = useMemo(() => {
    if (!hist) return null;
    const total = hist.bins.reduce((a, b) => a + b, hist.underflow + hist.overflow);
    return hist.bins.map((b) => (total === 0 ? 0 : (b / total) * 100));
  }, [hist]);
  const data = useMemo(() => {
    if (!hist || !bars) return null;
    const edge = (i: number) => hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns;
    return {
      labels: hist.bins.map((_, i) => rangeLabel(edge(i), edge(i + 1))),
      datasets: [
        {
          label: `${name} distribution`,
          data: bars,
          backgroundColor: iat ? IAT_BAR_COLOR : BAR_COLOR,
        },
      ],
    } as ChartData<'bar'>;
  }, [hist, bars, iat, name]);
  const options = useMemo(() => {
    if (!hist || !bars) return null;
    const annotations = hidden
      ? {}
      : percentileAnnotations(hist, iat ? IAT_PALETTE : DELAY_PALETTE, Math.max(0, ...bars));
    const base = chartOptions<'bar'>(theme, `${name} Range`, 'Probability (%)', HIST_ASPECT);
    return { ...base, plugins: { ...base.plugins, annotation: { annotations } } } as ChartOptions<'bar'>;
  }, [hist, bars, theme, iat, name, hidden]);

  if (error) {
    return (
      <small className="d-block mt-3 text-danger" data-testid={iat ? 'iat-error' : 'histogram-error'}>
        {name} histogram unavailable: {error}
      </small>
    );
  }
  if (!hist || !data || !options) {
    // The first visit: keep the room of the tables and the chart so that nothing below moves when the data arrives.
    return (
      <>
        <div className="hist-tables-placeholder" />
        <div className="chart-box chart-placeholder" />
      </>
    );
  }
  const stats = iat ? binStats(hist) : { mean: meanNs ?? hist.mean_ns, std: stdNs ?? hist.std_ns };
  return (
    <>
      <div className="d-flex justify-content-end align-items-center gap-2 mb-2">
        <button
          type="button"
          className={`btn btn-sm ${hidden ? 'btn-secondary' : 'btn-outline-secondary'}`}
          aria-pressed={hidden}
          onClick={onToggle}
        >
          {hidden ? 'Show percentiles' : 'Hide percentiles'}
        </button>
      </div>
      <HistogramTables hist={hist} name={name} meanNs={stats.mean} stdNs={stats.std} />
      <ChartBox label={`${name} histogram of link ${dir}`}>
        <Bar data={data} options={options} />
      </ChartBox>
    </>
  );
}
