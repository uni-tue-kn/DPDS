import {
  BarElement, CategoryScale, Chart as ChartJS, Filler, Legend, LinearScale, LineController, LineElement,
  LogarithmicScale, PointElement, Tooltip,
} from 'chart.js';

// Register once; importing this module is enough.
ChartJS.register(
  BarElement, CategoryScale, Filler, Legend, LinearScale, LineController, LineElement, LogarithmicScale, PointElement, Tooltip,
);
