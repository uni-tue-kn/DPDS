import type { ChartOptions } from 'chart.js';
import {
  ArcElement,
  BarElement,
  CategoryScale,
  Chart as ChartJS,
  DoughnutController,
  Filler,
  Legend,
  LinearScale,
  LineController,
  LineElement,
  PointElement,
  Title,
  Tooltip,
} from 'chart.js';
import Annotation from 'chartjs-plugin-annotation';
import type { Theme } from '../common/theme';

// Register once; importing this module is enough.
ChartJS.register(
  Annotation,
  ArcElement,
  BarElement,
  CategoryScale,
  DoughnutController,
  Filler,
  Legend,
  LinearScale,
  LineController,
  LineElement,
  PointElement,
  Title,
  Tooltip,
);

/** The colours of the rate chart: TX blue, RX orange-red, each with its translucent fill. */
export const TX_COLOR = 'rgb(53, 162, 235)';
export const TX_FILL = 'rgba(53, 162, 235, 0.5)';
export const RX_COLOR = 'rgb(183, 85, 40)';
export const RX_FILL = 'rgba(250, 122, 64, 0.5)';

/** Text and grid colours that read on the page background of the theme. */
export const chartInk = (theme: Theme) =>
  theme === 'dark'
    ? { text: '#e6e6e6', grid: 'rgba(255, 255, 255, 0.15)' }
    : { text: '#212529', grid: 'rgba(0, 0, 0, 0.1)' };

/** True when the user asked for reduced motion; false where `matchMedia` does not exist. */
const prefersReducedMotion = () =>
  typeof window.matchMedia === 'function' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;

/** `#rrggbb` as an `rgba()` string of the given alpha. */
export function hexAlpha(hex: string, alpha: number): string {
  const n = Number.parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}

/**
 * The options the line and bar charts share: wide and short by `aspectRatio`, titled axes, legend on top, y
 * from zero, no tooltips), text and grid in the colours of the theme. Chart.js animates and draws points by default;
 * animation is off for reduced motion.
 */
export function chartOptions<T extends 'line' | 'bar'>(
  theme: Theme,
  xTitle: string,
  yTitle: string,
  aspectRatio: number,
): ChartOptions<T> {
  const { text, grid } = chartInk(theme);
  const axis = (title: string) => ({
    title: { display: true, text: title, color: text },
    ticks: { color: text, autoSkip: true },
    grid: { color: grid },
  });
  return {
    ...(prefersReducedMotion() ? { animation: false } : {}),
    responsive: true,
    aspectRatio,
    scales: {
      x: axis(xTitle),
      y: { ...axis(yTitle), beginAtZero: true },
    },
    plugins: {
      legend: { display: true, position: 'top', labels: { color: text } },
      tooltip: { enabled: false },
    },
  } as unknown as ChartOptions<T>;
}
