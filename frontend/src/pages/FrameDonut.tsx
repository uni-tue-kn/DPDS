import type { ChartOptions } from 'chart.js';
import { useMemo } from 'react';
import { Doughnut } from 'react-chartjs-2';
import { formatCount } from '../common/format';
import { useTheme } from '../common/theme';

/** P4TG's frame size buckets and their colours (Visuals.tsx, frame_size_data). */
const SIZES = [
  { label: '0-63', color: 'rgb(255, 99, 132)' },
  { label: '64', color: 'rgb(54, 162, 235)' },
  { label: '65-127', color: 'rgb(255, 205, 86)' },
  { label: '128-255', color: 'rgb(18, 194, 0)' },
  { label: '256-511', color: 'rgb(178, 0, 255)' },
  { label: '512-1023', color: 'rgb(255, 104, 42)' },
  { label: '1024-1518', color: 'rgb(0, 0, 0)' },
  { label: '1519-21519', color: 'rgb(164, 0, 0)' },
] as const;

/** The one neutral segment shown instead of the buckets when no frame was received. */
const PLACEHOLDER = { label: 'No frames', color: '#adb5bd' } as const;

/** The distribution of the frames a link received by size; `counts` are the eight buckets. */
export default function FrameDonut({ counts }: { counts: number[] }) {
  const theme = useTheme();
  const [c0, c1, c2, c3, c4, c5, c6, c7] = counts;
  const total = counts.reduce((a, b) => a + b, 0);
  const empty = total === 0;
  // Memoised on the eight counts, not on the array: every poll brings a new array.
  const data = useMemo(
    () =>
      empty
        ? { labels: [PLACEHOLDER.label], datasets: [{ data: [1], backgroundColor: [PLACEHOLDER.color] }] }
        : {
            labels: SIZES.map((s) => s.label),
            datasets: [{ data: [c0, c1, c2, c3, c4, c5, c6, c7], backgroundColor: SIZES.map((s) => s.color) }],
          },
    [empty, c0, c1, c2, c3, c4, c5, c6, c7],
  );
  const options = useMemo(
    () =>
      ({
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        elements: { arc: { borderColor: theme === 'dark' ? '#212529' : '#ffffff' } },
        plugins: {
          legend: { display: false },
          tooltip: {
            callbacks: {
              label: (ctx: { label: string; parsed: number }) =>
                empty
                  ? PLACEHOLDER.label
                  : `${ctx.label}: ${formatCount(ctx.parsed)} frames (${((ctx.parsed / total) * 100).toFixed(2)} %)`,
            },
          },
        },
      }) as unknown as ChartOptions<'doughnut'>,
    [theme, empty, total],
  );

  return (
    <div className="d-flex flex-column align-items-center">
      <div className="donut-box" role="img" aria-label="Distribution of the received frames by size">
        <Doughnut data={data} options={options} />
      </div>
      {empty ? (
        <p className="text-center text-body-secondary mb-0">{PLACEHOLDER.label}</p>
      ) : (
        <ul className="list-unstyled d-flex flex-wrap justify-content-center column-gap-3 mt-2 mb-0">
          {SIZES.map((s) => (
            <li key={s.label} className="d-flex align-items-center gap-1">
              <span className="legend-swatch" style={{ backgroundColor: s.color }} />
              <span>{s.label}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
