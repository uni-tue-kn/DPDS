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
const PERCENTILES: [string, 'p50' | 'p90' | 'p99' | 'p999'][] = [
  ['p50', 'p50'],
  ['p90', 'p90'],
  ['p99', 'p99'],
  ['p99.9', 'p999'],
];

/** One vertical line per percentile at its place on the category axis (bin i is centred on i), if it has a value. */
function percentileLines(hist: HistogramView, textColor: string) {
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

/** The delay histogram of the whole run of link `dir`: its numbers in tables above the bars. */
export default function DelayHistogram({
  dir,
  meanNs,
  stdNs,
}: {
  dir: number;
  meanNs?: number | null;
  stdNs?: number | null;
}) {
  const theme = useTheme();
  const [hist, setHist] = useState<HistogramView | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const h = await api.getDelayHistogram(dir);
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
  }, [dir]);

  const data = useMemo(() => {
    if (!hist) return null;
    const labels = hist.bins.map((_, i) => formatDuration(hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns));
    return {
      labels,
      datasets: [
        { label: 'Packets', data: hist.bins, backgroundColor: BAR_COLOR, barPercentage: 1, categoryPercentage: 1 },
      ],
    } as ChartData<'bar'>;
  }, [hist]);
  const options = useMemo(() => {
    if (!hist) return null;
    const base = chartOptions<'bar'>(theme, 'Delay', 'Packets', false);
    return {
      ...base,
      plugins: { ...base.plugins, annotation: { annotations: percentileLines(hist, chartInk(theme).text) } },
    } as ChartOptions<'bar'>;
  }, [hist, theme]);

  if (error) {
    return (
      <small className="d-block mt-3 text-danger" data-testid="histogram-error">
        Delay histogram unavailable: {error}
      </small>
    );
  }
  if (!hist || !data || !options) return null;
  return (
    <>
      <HistogramTables hist={hist} meanNs={meanNs} stdNs={stdNs} />
      <ChartBox label={`Delay histogram of link ${dir}`}>
        <Bar data={data} options={options} />
      </ChartBox>
    </>
  );
}
