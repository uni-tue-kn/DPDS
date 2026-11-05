import type { ChartOptions } from 'chart.js';
import { useId, useMemo } from 'react';
import { OverlayTrigger, Tooltip } from 'react-bootstrap';
import { Doughnut } from 'react-chartjs-2';
import type { Counters, HistoryTotals } from '../api/types';
import { formatCount } from '../common/format';
import { useTheme } from '../common/theme';

/** One segment per cause, in this order, in a soft palette of clearly different colours (one red only; the frame size donut uses it without the red). */
export const CAUSES = [
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
    label: 'NIC TX ring full',
    color: '#c0392b',
    help: 'Packets the NIC would not accept for sending because its transmit ring was full (tx_drops).',
  },
  {
    label: 'NIC RX missed',
    color: '#a29bfe',
    help: 'Packets the NIC dropped because its receive queues were full (rx_missed_errors).',
  },
  {
    label: 'RX buffers exhausted',
    color: '#fd79a8',
    help: 'Packets lost because no packet buffer (mbuf) was free to receive them (rx_mbuf_allocation_errors).',
  },
  {
    label: 'NIC RX port buffer full',
    color: '#00cec9',
    help: "Packets the NIC's port buffer discarded because it was full or congested, summed over all priorities (rx_prio*_buf_discard_packets, rx_prio*_cong_discard_packets).",
  },
  {
    label: 'NIC RX port discards',
    color: '#55efc4',
    help: 'Packets the NIC port discarded on receive because it had no buffers left (rx_phy_discard_packets).',
  },
  {
    label: 'NIC TX port discards',
    color: '#636e72',
    help: 'Packets the NIC accepted for sending but dropped before they reached the wire (tx_phy_discard_packets).',
  },
] as const;

/** The cumulative drops of one link by cause, in the order of `CAUSES`. */
export interface DropCounts {
  loss: number;
  limiter: number;
  bufferFull: number;
  nicTx: number;
  missed: number;
  nombuf: number;
  portBuffer: number;
  rxPhy: number;
  txPhy: number;
}

/** From the live counters of `GET /api/stats`. */
export const dropsOfCounters = (c: Counters): DropCounts => ({
  loss: c.loss_drops,
  limiter: c.limiter_drops,
  bufferFull: c.buffer_full_drops,
  nicTx: c.tx_drops,
  missed: c.nic_rx_missed,
  nombuf: c.nic_rx_nombuf,
  portBuffer: c.nic_rx_port_buffer_discards,
  rxPhy: c.nic_rx_phy_discards,
  txPhy: c.nic_tx_phy_discards,
});

/** From the run's final counters of `GET /api/stats/history`. */
export const dropsOfTotals = (t: HistoryTotals): DropCounts => ({
  loss: t.loss_drops,
  limiter: t.limiter_drops,
  bufferFull: t.buffer_full_drops,
  nicTx: t.nic_tx_drops,
  missed: t.nic_rx_missed,
  nombuf: t.nic_rx_nombuf,
  portBuffer: t.nic_rx_port_buffer_discards,
  rxPhy: t.nic_rx_phy_discards,
  txPhy: t.nic_tx_phy_discards,
});

/** All causes together; 0 means nothing was dropped. */
export const totalDrops = (d: DropCounts) =>
  d.loss + d.limiter + d.bufferFull + d.nicTx + d.missed + d.nombuf + d.portBuffer + d.rxPhy + d.txPhy;

/** The one neutral segment shown instead of the causes when nothing was dropped. */
const PLACEHOLDER = { label: 'No drops', color: '#adb5bd' } as const;

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

/** The distribution of a link's drops by cause, with a legend whose hover help names the raw counters. */
export default function LossDonut({ drops }: { drops: DropCounts }) {
  const theme = useTheme();
  const { loss, limiter, bufferFull, nicTx, missed, nombuf, portBuffer, rxPhy, txPhy } = drops;
  const empty = totalDrops(drops) === 0;
  const data = useMemo(
    () =>
      empty
        ? { labels: [PLACEHOLDER.label], datasets: [{ data: [1], backgroundColor: [PLACEHOLDER.color] }] }
        : {
            labels: CAUSES.map((c) => c.label),
            datasets: [
              {
                data: [loss, limiter, bufferFull, nicTx, missed, nombuf, portBuffer, rxPhy, txPhy],
                backgroundColor: CAUSES.map((c) => c.color),
              },
            ],
          },
    [empty, loss, limiter, bufferFull, nicTx, missed, nombuf, portBuffer, rxPhy, txPhy],
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
                empty
                  ? PLACEHOLDER.label
                  : hoverText(
                      ctx.label,
                      ctx.parsed,
                      ctx.dataset.data.reduce((a, b) => a + b, 0),
                    ),
            },
          },
        },
      }) as unknown as ChartOptions<'doughnut'>,
    [theme, empty],
  );

  return (
    <div className="d-flex flex-wrap justify-content-center align-items-center gap-4">
      <div>
        <div className="donut-box" role="img" aria-label="Distribution of the dropped packets by cause">
          <Doughnut data={data} options={options} />
        </div>
        {empty && <p className="text-center text-body-secondary mb-0">{PLACEHOLDER.label}</p>}
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
