import type { ChartData, ChartOptions } from 'chart.js';
import { useEffect, useMemo, useState } from 'react';
import { Bar } from 'react-chartjs-2';
import * as api from '../api/client';
import type { HistogramView } from '../api/types';
import { errorMessage, formatDuration } from '../common/format';
import { startPolling } from '../common/polling';
import { useTheme } from '../common/theme';
import { chartInk, chartOptions } from '../components/charts';
import ChartBox from './ChartBox';
import HistogramTables from './HistogramTables';

const POLL_MS = 1000;
const BAR_COLOR = '#e74c3c';
const IAT_BAR_COLOR = '#3498db';
const PERCENTILES: [string, 'p50' | 'p90' | 'p99' | 'p999'][] = [
  ['p50', 'p50'],
  ['p90', 'p90'],
  ['p99', 'p99'],
  ['p99.9', 'p999'],
];

/** One vertical line per percentile at its place on the category axis (bin i is centred on i), if it has a value. */
export function percentileLines(hist: HistogramView, textColor: string) {
  const p = hist.percentiles;
  const lines: Record<string, unknown> = {};
  if (!p) return lines;
  PERCENTILES.forEach(([label, key], i) => {
    const value = p[key];
    if (value == null) return;
    const x = (value - hist.lo_ns) / hist.bin_ns - 0.5;
    lines[key] = {
      type: 'line',
      xMin: x,
      xMax: x,
      borderColor: textColor,
      borderDash: [6, 4],
      borderWidth: 1,
      label: { display: true, content: label, position: 'start', yAdjust: i * 22 },
    };
  });
  return lines;
}

/** The unit of the IAT's x axis: the largest one the histogram's upper edge reaches. */
const UNITS = [
  { name: 'ns', ns: 1 },
  { name: 'µs', ns: 1e3 },
  { name: 'ms', ns: 1e6 },
  { name: 's', ns: 1e9 },
] as const;

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
 * IAT histogram of the link instead (blue bars, time-unit axis, mean and σ from the bins, no live stats).
 */
export default function DelayHistogram({
  dir,
  active = true,
  kind = 'delay',
  meanNs,
  stdNs,
  hidden,
  onToggle,
}: {
  dir: number;
  /** Whether the link's tab is shown: a hidden histogram keeps what it has but does not poll. */
  active?: boolean;
  kind?: 'delay' | 'iat';
  meanNs?: number | null;
  stdNs?: number | null;
  /** The percentile keys whose line is switched off; the owner keeps them so that they outlive this view and the tab. */
  hidden: ReadonlySet<string>;
  onToggle: (key: string) => void;
}) {
  const iat = kind === 'iat';
  const name = iat ? 'IAT' : 'Delay';
  const theme = useTheme();
  const ink = chartInk(theme).text;
  const [hist, setHist] = useState<HistogramView | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    if (!active) return;
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const h = await (iat ? api.getIatHistogram(dir) : api.getDelayHistogram(dir));
          if (!alive) return;
          setHist(h);
          setError(undefined);
        } catch (err) {
          if (!alive) return;
          setHist(null);
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
  }, [dir, iat, active]);

  const unit = useMemo(() => {
    const top = hist ? hist.lo_ns + hist.bins.length * hist.bin_ns : 0;
    return [...UNITS].reverse().find((u) => top >= u.ns) ?? UNITS[0];
  }, [hist]);
  const data = useMemo(() => {
    if (!hist) return null;
    const edge = (i: number) => hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns;
    const labels = hist.bins.map((_, i) =>
      iat ? `${Number((edge(i) / unit.ns).toFixed(2))}` : formatDuration(edge(i)),
    );
    return {
      labels,
      datasets: [
        {
          label: 'Packets',
          data: hist.bins,
          backgroundColor: iat ? IAT_BAR_COLOR : BAR_COLOR,
          barPercentage: 1,
          categoryPercentage: 1,
        },
      ],
    } as ChartData<'bar'>;
  }, [hist, iat, unit]);
  const options = useMemo(() => {
    if (!hist) return null;
    const lines = Object.fromEntries(Object.entries(percentileLines(hist, ink)).filter(([key]) => !hidden.has(key)));
    const base = iat
      ? chartOptions<'bar'>(theme, `IAT (${unit.name})`, '#Packets', false)
      : chartOptions<'bar'>(theme, 'Delay', 'Packets', false);
    return {
      ...base,
      plugins: {
        ...base.plugins,
        ...(iat ? { title: { display: true, text: 'IAT', color: ink } } : {}),
        annotation: { annotations: lines },
      },
    } as ChartOptions<'bar'>;
  }, [hist, theme, ink, iat, unit, hidden]);

  if (error) {
    return (
      <small className="d-block mt-3 text-danger" data-testid={iat ? 'iat-error' : 'histogram-error'}>
        {name} histogram unavailable: {error}
      </small>
    );
  }
  if (!hist || !data || !options) return null;
  const stats = iat ? binStats(hist) : { mean: meanNs ?? hist.mean_ns, std: stdNs ?? hist.std_ns };
  return (
    <>
      <HistogramTables hist={hist} name={name} meanNs={stats.mean} stdNs={stats.std} />
      <div className="d-flex justify-content-center gap-3 mt-3">
        {PERCENTILES.filter(([, key]) => hist.percentiles?.[key] != null).map(([label, key]) => {
          const shown = !hidden.has(key);
          return (
            <button
              key={key}
              type="button"
              className="btn btn-link btn-sm p-0 text-reset text-decoration-none"
              aria-pressed={shown}
              onClick={() => onToggle(key)}
            >
              <span
                className="d-inline-block me-1 align-middle"
                style={{ width: 32, height: 10, backgroundColor: ink }}
              />
              <span style={{ textDecoration: shown ? 'none' : 'line-through' }}>{label}</span>
            </button>
          );
        })}
      </div>
      <ChartBox label={`${name} histogram of link ${dir}`}>
        <Bar data={data} options={options} />
      </ChartBox>
    </>
  );
}
