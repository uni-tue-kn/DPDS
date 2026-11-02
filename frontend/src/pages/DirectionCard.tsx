import { useEffect, useState } from 'react';
import { Badge, Card, Col, Row } from 'react-bootstrap';
import { Bar, Line } from 'react-chartjs-2';
import '../components/charts';
import { startPolling } from '../common/polling';
import {
  directionLabel, errorMessage, formatBps, formatDuration, formatInt, formatPct, formatPps,
} from '../common/format';
import * as api from '../api/client';
import type { DirectionPlan, DirectionStats, HistogramView } from '../api/types';

/** One sample of the rate/drop charts (taken at every stats poll). */
export interface Sample {
  t: number;
  rx_bps: number;
  tx_bps: number;
  loss_pct: number;
  limiter_pct: number;
  buffer_full_pct: number;
  nic_tx_pct: number;
  nic_rx_pct: number;
}

const POLL_MS = 1000;
const COLORS = ['#e74c3c', '#2c3e50', '#27ae60', '#ffc107', '#3498db'];

const chartOptions = {
  animation: false,
  responsive: true,
  maintainAspectRatio: false,
  elements: { point: { radius: 0 } },
  scales: { x: { ticks: { maxTicksLimit: 8 } } },
} as const;

function Stat({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

/** A chart as an image for assistive technology: the canvas itself has no text. */
function ChartBox({ label, children }: { label: string; children: React.ReactNode }) {
  return <div className="chart-box" role="img" aria-label={label}>{children}</div>;
}

function Histogram({ dir }: { dir: number }) {
  const [hist, setHist] = useState<HistogramView | null>(null);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let alive = true;
    const stop = startPolling(async () => {
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
    }, POLL_MS, true);
    return () => {
      alive = false;
      stop();
    };
  }, [dir]);

  if (error) {
    return <small className="d-block mt-3 text-danger" data-testid="histogram-error">Delay histogram unavailable: {error}</small>;
  }
  if (!hist) return null;
  const labels = hist.bins.map((_, i) => formatDuration(hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns));
  const data = {
    labels,
    datasets: [{ label: 'Packets', data: hist.bins, backgroundColor: COLORS[0], barPercentage: 1, categoryPercentage: 1 }],
  };
  return (
    <>
      <h3 className="h6 mt-3">Delay histogram</h3>
      <ChartBox label={`Delay histogram of direction ${dir}`}>
        <Bar data={data} options={{ ...chartOptions, plugins: { legend: { display: false } } }} />
      </ChartBox>
      <small className="text-body-secondary">
        Underflow: {hist.underflow} &middot; Overflow: {hist.overflow}
      </small>
    </>
  );
}

export default function DirectionCard({ stats, plan, history }: {
  stats: DirectionStats;
  plan?: DirectionPlan;
  history: Sample[];
}) {
  const labels = history.map((s) => new Date(s.t).toLocaleTimeString());
  const rateData = {
    labels,
    datasets: [
      { label: 'RX Gb/s', data: history.map((s) => s.rx_bps / 1e9), borderColor: COLORS[0], backgroundColor: COLORS[0] },
      { label: 'TX Gb/s', data: history.map((s) => s.tx_bps / 1e9), borderColor: COLORS[1], backgroundColor: COLORS[1] },
    ],
  };
  const drops: [string, (s: Sample) => number][] = [
    ['Loss', (s) => s.loss_pct],
    ['Limiter', (s) => s.limiter_pct],
    ['Buffer full', (s) => s.buffer_full_pct],
    ['NIC TX', (s) => s.nic_tx_pct],
    ['NIC RX', (s) => s.nic_rx_pct],
  ];
  const dropData = {
    labels,
    datasets: drops.map(([label, pick], i) => ({
      label: `${label} %`, data: history.map(pick), borderColor: COLORS[i], backgroundColor: COLORS[i],
    })),
  };

  const rx = stats.rx ?? plan?.rx;
  const tx = stats.tx ?? plan?.tx;
  return (
    <Card className="mb-3" data-testid={`direction-card-${stats.dir}`}>
      <Card.Header className="d-flex justify-content-between align-items-center">
        <span>
          {directionLabel(stats.dir, rx, tx)} <small className="text-body-secondary">{stats.link ?? plan?.link}</small>
        </span>
        {plan && <Badge pill bg="light" text="dark">lcore {plan.lcore}</Badge>}
      </Card.Header>
      <Card.Body>
        <Row>
          <Col lg={5}>
            <dl className="stat-list">
              <Stat label="RX rate">{formatPps(stats.rx_pps)} / {formatBps(stats.rx_bps)}</Stat>
              <Stat label="TX rate">{formatPps(stats.tx_pps)} / {formatBps(stats.tx_bps)}</Stat>
              <Stat label="Loss drops">{formatPct(stats.loss_pct)}</Stat>
              <Stat label="Limiter drops">{formatPct(stats.limiter_pct)}</Stat>
              <Stat label="Buffer full drops">{formatPct(stats.buffer_full_pct)}</Stat>
              <Stat label="NIC TX drops">{formatPct(stats.nic_tx_pct)}</Stat>
              <Stat label="NIC RX drops">{formatPct(stats.nic_rx_pct)}</Stat>
              <Stat label="Mean delay">
                {stats.delay_mean_ns == null || stats.delay_std_ns == null
                  ? 'n/a'
                  : `${formatDuration(stats.delay_mean_ns)} ± ${formatDuration(stats.delay_std_ns)}`}
              </Stat>
              <Stat label="Delay min / max">
                {formatDuration(stats.delay_min_ns)} / {formatDuration(stats.delay_max_ns)}
              </Stat>
              <Stat label="Lateness">
                mean {formatDuration(stats.lateness_mean_ns)} / max {formatDuration(stats.lateness_max_ns)}
              </Stat>
              <Stat label="Buffer max">{formatInt(stats.buffer_len_max)}</Stat>
              <Stat label="First loss">
                {stats.first_loss_reason
                  ? [stats.first_loss_reason, stats.first_loss_detail, formatDuration(stats.first_loss_relative_ns)]
                    .filter(Boolean).join(', ')
                  : 'n/a'}
              </Stat>
            </dl>
          </Col>
          <Col lg={7}>
            <h3 className="h6">Rate</h3>
            <ChartBox label={`Rate chart of direction ${stats.dir}`}><Line data={rateData} options={chartOptions} /></ChartBox>
            <h3 className="h6 mt-3">Drops</h3>
            <ChartBox label={`Drops chart of direction ${stats.dir}`}><Line data={dropData} options={chartOptions} /></ChartBox>
            {stats.delay_stats && <Histogram dir={stats.dir} />}
          </Col>
        </Row>
      </Card.Body>
    </Card>
  );
}
