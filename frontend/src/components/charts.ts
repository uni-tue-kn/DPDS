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
import type { ChartOptions } from 'chart.js';
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

/** The colours of the P4TG charts: RX blue, TX orange-brown. The fill is the same colour, translucent. */
export const RX_COLOR = 'rgb(53, 162, 235)';
export const RX_FILL = 'rgba(53, 162, 235, 0.5)';
export const TX_COLOR = 'rgb(183, 85, 40)';
export const TX_FILL = 'rgba(183, 85, 40, 0.5)';

/** Text and grid colours that read on the page background of the theme. */
export const chartInk = (theme: Theme) =>
  theme === 'dark'
    ? { text: '#e6e6e6', grid: 'rgba(255, 255, 255, 0.15)' }
    : { text: '#212529', grid: 'rgba(0, 0, 0, 0.1)' };

/**
 * The options every chart shares: P4TG look (wide and short, titled axes, legend on top, y from zero), text and grid in
 * the colours of the theme, and hover tooltips that work without visible points.
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
    ticks: { color: text, maxTicksLimit: 8 },
    grid: { color: grid },
  });
  return {
    animation: false,
    responsive: true,
    maintainAspectRatio: false,
    elements: { point: { radius: 0 } },
    interaction: { mode: 'index', intersect: false },
    scales: {
      x: axis(xTitle),
      y: { ...axis(yTitle), beginAtZero: true },
    },
    plugins: { legend: { display: showLegend, position: 'top', labels: { color: text } } },
  } as unknown as ChartOptions<T>;
}
