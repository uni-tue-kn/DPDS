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

/** The colours of P4TG's rate chart (Visuals.tsx rateDatasets): TX blue, RX orange-red, each with its translucent fill. */
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

/**
 * The options every chart shares: P4TG look (wide and short, titled axes, legend on top, y from zero), text and grid in
 * the colours of the theme. Chart.js animates and draws points by default; animation is off for reduced motion.
 */
export function chartOptions<T extends 'line' | 'bar'>(
  theme: Theme,
  xTitle: string,
  yTitle: string,
  showLegend = true,
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
    maintainAspectRatio: false,
    interaction: { mode: 'index', intersect: false },
    scales: {
      x: axis(xTitle),
      y: { ...axis(yTitle), beginAtZero: true },
    },
    plugins: { legend: { display: showLegend, position: 'top', labels: { color: text } } },
  } as unknown as ChartOptions<T>;
}
