import type { ChartData, ChartOptions } from 'chart.js';
import { useEffect, useMemo, useState } from 'react';
import { Bar } from 'react-chartjs-2';
import * as api from '../api/client';
import type { HistogramView } from '../api/types';
import { errorMessage } from '../common/format';
import { startPolling } from '../common/polling';
import { useTheme } from '../common/theme';
import { chartInk, chartOptions } from '../components/charts';
import ChartBox from './ChartBox';
import { percentileLines } from './DelayHistogram';

const POLL_MS = 1000;
const BAR_COLOR = '#3498db';

/** The unit of the x axis: the largest one the histogram's upper edge reaches. */
const UNITS = [
  { name: 'ns', ns: 1 },
  { name: 'µs', ns: 1e3 },
  { name: 'ms', ns: 1e6 },
  { name: 's', ns: 1e9 },
] as const;

/** The IAT (the gap between the departures of consecutive packets) of the run of link `dir`, as a bar chart. */
export default function IatHistogram({ dir }: { dir: number }) {
  const theme = useTheme();
  const [hist, setHist] = useState<HistogramView | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const h = await api.getIatHistogram(dir);
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

  const unit = useMemo(() => {
    const top = hist ? hist.lo_ns + hist.bins.length * hist.bin_ns : 0;
    return [...UNITS].reverse().find((u) => top >= u.ns) ?? UNITS[0];
  }, [hist]);
  const data = useMemo(() => {
    if (!hist) return null;
    const labels = hist.bins.map(
      (_, i) => `${Number(((hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns) / unit.ns).toFixed(2))}`,
    );
    return {
      labels,
      datasets: [
        { label: 'Packets', data: hist.bins, backgroundColor: BAR_COLOR, barPercentage: 1, categoryPercentage: 1 },
      ],
    } as ChartData<'bar'>;
  }, [hist, unit]);
  const options = useMemo(() => {
    if (!hist) return null;
    const base = chartOptions<'bar'>(theme, `IAT (${unit.name})`, '#Packets', false);
    const ink = chartInk(theme).text;
    return {
      ...base,
      plugins: {
        ...base.plugins,
        title: { display: true, text: 'IAT', color: ink },
        annotation: { annotations: percentileLines(hist, ink) },
      },
    } as ChartOptions<'bar'>;
  }, [hist, theme, unit]);

  if (error) {
    return (
      <small className="d-block mt-3 text-danger" data-testid="iat-error">
        IAT histogram unavailable: {error}
      </small>
    );
  }
  if (!data || !options) return null;
  return (
    <ChartBox label={`IAT histogram of link ${dir}`}>
      <Bar data={data} options={options} />
    </ChartBox>
  );
}
