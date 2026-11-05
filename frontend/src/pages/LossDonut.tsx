import type { ChartOptions } from 'chart.js';
import { useId, useMemo } from 'react';
import { OverlayTrigger, Tooltip } from 'react-bootstrap';
import { Doughnut } from 'react-chartjs-2';
import type { Counters, HistoryTotals } from '../api/types';
import { formatCount } from '../common/format';
import { useTheme } from '../common/theme';

/** One segment per cause, in this order; the loss model is neutral (it is intended), the overload causes are warm. */
const CAUSES = [
  {
    label: 'Loss model',
    color: '#6c8ebf',
    help: 'Packets dropped on purpose by the configured loss model (random or Gilbert-Elliott).',
  },
  {
    label: 'Rate limiter',
    color: '#f6b73c',
    help: "Packets the limiter refused: the Spacer's backlog or the token bucket's budget was exceeded.",
  },
  {
    label: 'Buffer full',
    color: '#e67e22',
    help: 'Packets that arrived while the delay buffer already held its maximum number of packets.',
  },
  {
    label: 'NIC TX ring full (tx_drops)',
    color: '#e74c3c',
    help: 'Packets the NIC would not accept for sending because its transmit ring was full.',
  },
  {
    label: 'NIC RX missed (imissed)',
    color: '#c0392b',
    help: 'Packets the NIC dropped because its receive queues were full.',
  },
  {
    label: 'RX buffers exhausted (rx_nombuf)',
    color: '#8e2b1f',
    help: 'Packets lost because no packet buffer (mbuf) was free to receive them.',
  },
] as const;

/** The cumulative drops of one link by cause, in the order of `CAUSES`. */
export interface DropCounts {
  loss: number;
  limiter: number;
  bufferFull: number;
  nicTx: number;
  imissed: number;
  nombuf: number;
}

/** From the live counters of `GET /api/stats`. */
export const dropsOfCounters = (c: Counters): DropCounts => ({
  loss: c.loss_drops,
  limiter: c.limiter_drops,
  bufferFull: c.buffer_full_drops,
  nicTx: c.tx_drops,
  imissed: c.nic_rx_imissed,
  nombuf: c.nic_rx_nombuf,
});

/** From the run's final counters of `GET /api/stats/history`. */
export const dropsOfTotals = (t: HistoryTotals): DropCounts => ({
  loss: t.loss_drops,
  limiter: t.limiter_drops,
  bufferFull: t.buffer_full_drops,
  nicTx: t.nic_tx_drops,
  imissed: t.nic_rx_imissed,
  nombuf: t.nic_rx_nombuf,
});

/** All causes together; 0 means nothing was dropped. */
export const totalDrops = (d: DropCounts) => d.loss + d.limiter + d.bufferFull + d.nicTx + d.imissed + d.nombuf;

/** The hover text of a segment: its count and its share of all drops. */
function hoverText(label: string, count: number, total: number): string {
  const share = total > 0 ? (count / total) * 100 : 0;
  return `${label}: ${formatCount(count)} packets (${share.toFixed(2)} % of drops)`;
}

function CauseInfo({ label, help }: { label: string; help: string }) {
  const id = useId();
  return (
    <OverlayTrigger
      placement="top"
      trigger={['hover', 'focus']}
      overlay={
        <Tooltip id={id} className="info-tip-text">
          {help}
        </Tooltip>
      }
    >
      <button type="button" className="info-tip" aria-label={`About ${label}`}>
        <i className="bi bi-info-circle" aria-hidden="true" />
      </button>
    </OverlayTrigger>
  );
}

/** The distribution of a link's drops by cause, with a legend that explains the raw counter names. */
export default function LossDonut({ drops }: { drops: DropCounts }) {
  const theme = useTheme();
  const { loss, limiter, bufferFull, nicTx, imissed, nombuf } = drops;
  const data = useMemo(
    () => ({
      labels: CAUSES.map((c) => c.label),
      datasets: [
        { data: [loss, limiter, bufferFull, nicTx, imissed, nombuf], backgroundColor: CAUSES.map((c) => c.color) },
      ],
    }),
    [loss, limiter, bufferFull, nicTx, imissed, nombuf],
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
              label: (ctx: { label: string; parsed: number; dataset: { data: number[] } }) =>
                hoverText(
                  ctx.label,
                  ctx.parsed,
                  ctx.dataset.data.reduce((a, b) => a + b, 0),
                ),
            },
          },
        },
      }) as unknown as ChartOptions<'doughnut'>,
    [theme],
  );

  if (totalDrops(drops) === 0) return <p className="text-center text-body-secondary mb-0">No drops</p>;
  return (
    <div className="d-flex flex-wrap justify-content-center align-items-center gap-4">
      <div className="donut-box" role="img" aria-label="Distribution of the dropped packets by cause">
        <Doughnut data={data} options={options} />
      </div>
      <ul className="list-unstyled mb-0" data-testid="donut-legend">
        {CAUSES.map((c) => (
          <li key={c.label} className="d-flex align-items-center gap-2">
            <span className="legend-swatch" data-testid="legend-swatch" style={{ backgroundColor: c.color }} />
            <span>{c.label}</span>
            <CauseInfo label={c.label} help={c.help} />
          </li>
        ))}
      </ul>
    </div>
  );
}
