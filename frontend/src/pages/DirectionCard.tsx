import { Fragment, useEffect, useState } from 'react';
import type { ChartData, ChartOptions } from 'chart.js';
import { Badge, Card, Col, Form, Row } from 'react-bootstrap';
import { Bar, Line } from 'react-chartjs-2';
import { chartInk, chartOptions, RX_COLOR, RX_FILL, TX_COLOR, TX_FILL } from '../components/charts';
import { useTheme } from '../common/theme';
import { startPolling } from '../common/polling';
import { errorMessage, formatBps, formatDuration, formatInt, formatPct, formatPps, linkLabel } from '../common/format';
import * as api from '../api/client';
import type { PortLabeler } from '../common/portLabel';
import type { DirectionPlan, DirectionStats, HistogramView, Percentiles as PercentilesDto } from '../api/types';

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
const COLORS = ['#e74c3c', '#2c3e50', '#27ae60'];

type View = 'rate' | 'drops' | 'histogram';

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
  return (
    <div className="chart-box" role="img" aria-label={label}>
      {children}
    </div>
  );
}

const WINDOWS: [number, string][] = [
  [0, 'Since start'],
  [10, '10 s'],
  [60, '60 s'],
  [300, '300 s'],
];

const PERCENTILES: [string, 'p50' | 'p90' | 'p99' | 'p999'][] = [
  ['p50', 'p50'],
  ['p90', 'p90'],
  ['p99', 'p99'],
  ['p99.9', 'p999'],
];

/** p50..p99.9; a value that fell into the under/overflow is only a bound of the true percentile. */
function Percentiles({ p }: { p: PercentilesDto }) {
  return (
    <dl className="stat-list mt-2" data-testid="percentiles">
      {PERCENTILES.map(([label, key]) => {
        const clamped = p[`${key}_clamped`] ?? undefined;
        return (
          <Fragment key={key}>
            <dt>{label}</dt>
            <dd
              data-clamped={clamped}
              title={
                clamped &&
                `Outside the histogram range (${clamped}): the real value is ${clamped === 'overflow' ? 'higher' : 'lower'}`
              }
            >
              {clamped === 'underflow' && '≤ '}
              {clamped === 'overflow' && '≥ '}
              {formatDuration(p[key])}
            </dd>
          </Fragment>
        );
      })}
    </dl>
  );
}

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

function Histogram({ dir }: { dir: number }) {
  const theme = useTheme();
  const [hist, setHist] = useState<HistogramView | null>(null);
  const [error, setError] = useState<string>();
  const [windowS, setWindowS] = useState(0);
  const [logScale, setLogScale] = useState(false);
  const [showExpected, setShowExpected] = useState(true);

  // A new window restarts the poll, which fetches at once; the old reply of a superseded window is dropped.
  useEffect(() => {
    let alive = true;
    const stop = startPolling(
      async () => {
        try {
          const h = await (windowS === 0 ? api.getDelayHistogram(dir) : api.getDelayHistogram(dir, windowS));
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
  }, [dir, windowS]);

  const controls = (
    <div className="d-flex flex-wrap align-items-center gap-3 mb-2">
      <Form.Group controlId={`hist-window-${dir}`} className="d-flex align-items-center gap-2">
        <Form.Label className="mb-0">Histogram window</Form.Label>
        <Form.Select
          size="sm"
          className="w-auto"
          value={String(windowS)}
          onChange={(e) => setWindowS(Number(e.target.value))}
        >
          {WINDOWS.map(([n, label]) => (
            <option key={n} value={n}>
              {label}
            </option>
          ))}
        </Form.Select>
      </Form.Group>
      <Form.Check
        type="checkbox"
        id={`hist-log-${dir}`}
        label="Log scale"
        checked={logScale}
        onChange={(e) => setLogScale(e.target.checked)}
      />
      <Form.Check
        type="checkbox"
        id={`hist-expected-${dir}`}
        label="Show expected distribution"
        checked={showExpected}
        onChange={(e) => setShowExpected(e.target.checked)}
      />
    </div>
  );

  if (error) {
    return (
      <>
        {controls}
        <small className="d-block mt-3 text-danger" data-testid="histogram-error">
          Delay histogram unavailable: {error}
        </small>
      </>
    );
  }
  if (!hist) return null;
  const labels = hist.bins.map((_, i) => formatDuration(hist.edges_ns[i] ?? hist.lo_ns + i * hist.bin_ns));
  const overlay = showExpected && hist.expected !== undefined;
  const expectedLabel =
    `Expected (${hist.expected_model === 'normal_approximation' ? 'normal approximation' : 'exact'})` +
    (hist.expected_note ? ` - ${hist.expected_note}` : '');
  const datasets: unknown[] = [
    { label: 'Packets', data: hist.bins, backgroundColor: COLORS[0], barPercentage: 1, categoryPercentage: 1 },
  ];
  if (overlay) {
    datasets.push({
      type: 'line',
      label: expectedLabel,
      data: hist.expected,
      borderColor: COLORS[1],
      backgroundColor: COLORS[1],
      borderWidth: 2,
    });
  }
  // The mixed bar + line dataset list is outside what the `Bar` typings allow.
  const data = { labels, datasets } as unknown as ChartData<'bar'>;
  const base = chartOptions<'bar'>(theme, 'Delay', 'Packets', {
    type: logScale ? 'logarithmic' : 'linear',
    showLegend: overlay,
  });
  const options = {
    ...base,
    plugins: {
      ...base.plugins,
      annotation: { annotations: percentileLines(hist, chartInk(theme).text) },
    },
  } as ChartOptions<'bar'>;
  return (
    <>
      {controls}
      <ChartBox label={`Delay histogram of direction ${dir}`}>
        <Bar data={data} options={options} />
      </ChartBox>
      <small className="text-body-secondary">
        Underflow: {hist.underflow} &middot; Overflow: {hist.overflow}
      </small>{' '}
      {hist.percentiles && <Percentiles p={hist.percentiles} />}
    </>
  );
}

export default function DirectionCard({
  stats,
  plan,
  label,
  history,
}: {
  stats: DirectionStats;
  plan?: DirectionPlan;
  label: PortLabeler;
  history: Sample[];
}) {
  const theme = useTheme();
  const [chosen, setChosen] = useState<View>('rate');
  // A link whose delay statistics are off has no histogram, whatever was chosen before.
  const view = chosen === 'histogram' && !stats.delay_stats ? 'rate' : chosen;
  const views: [View, string][] = [
    ['rate', 'Rate'],
    ['drops', 'Drops'],
    ...(stats.delay_stats ? ([['histogram', 'Delay histogram']] as [View, string][]) : []),
  ];
  const labels = history.map((s) => new Date(s.t).toLocaleTimeString());
  const rateData = {
    labels,
    datasets: [
      {
        label: 'RX',
        data: history.map((s) => s.rx_bps / 1e9),
        borderColor: RX_COLOR,
        backgroundColor: RX_FILL,
        fill: true,
      },
      {
        label: 'TX',
        data: history.map((s) => s.tx_bps / 1e9),
        borderColor: TX_COLOR,
        backgroundColor: TX_FILL,
        fill: true,
      },
    ],
  };
  // The loss model is intended in an emulator; everything else is the emulator running out of room.
  const dropData = {
    labels,
    datasets: [
      {
        label: 'Model loss %',
        data: history.map((s) => s.loss_pct),
        borderColor: COLORS[2],
        backgroundColor: COLORS[2],
      },
      {
        label: 'Overload drops %',
        data: history.map((s) => s.limiter_pct + s.buffer_full_pct + s.nic_tx_pct + s.nic_rx_pct),
        borderColor: COLORS[0],
        backgroundColor: COLORS[0],
      },
    ],
  };

  const rx = stats.rx ?? plan?.rx;
  const tx = stats.tx ?? plan?.tx;
  return (
    <Card className="mb-3" data-testid={`direction-card-${stats.dir}`}>
      <Card.Header className="d-flex justify-content-between align-items-center">
        <span>{linkLabel(stats.link ?? plan?.link, rx, tx, label)}</span>
        {plan && (
          <Badge pill bg="light" text="dark">
            lcore {plan.lcore}
          </Badge>
        )}
      </Card.Header>
      <Card.Body>
        <Row>
          <Col lg={5}>
            <dl className="stat-list">
              <Stat label="RX rate">
                {formatPps(stats.rx_pps)} / {formatBps(stats.rx_bps)}
              </Stat>
              <Stat label="TX rate">
                {formatPps(stats.tx_pps)} / {formatBps(stats.tx_bps)}
              </Stat>
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
                      .filter(Boolean)
                      .join(', ')
                  : 'n/a'}
              </Stat>
            </dl>
          </Col>
          <Col lg={7}>
            <Form.Group role="radiogroup" aria-label="Chart" className="mb-2">
              {views.map(([v, text]) => (
                <Form.Check
                  key={v}
                  inline
                  type="radio"
                  name={`chart-${stats.dir}`}
                  id={`chart-${stats.dir}-${v}`}
                  label={text}
                  checked={view === v}
                  onChange={() => setChosen(v)}
                />
              ))}
            </Form.Group>
            {view === 'rate' && (
              <ChartBox label={`Rate chart of direction ${stats.dir}`}>
                <Line data={rateData} options={chartOptions<'line'>(theme, 'Time', 'Rate (Gbit/s)')} />
              </ChartBox>
            )}
            {view === 'drops' && (
              <ChartBox label={`Drops chart of direction ${stats.dir}`}>
                <Line data={dropData} options={chartOptions<'line'>(theme, 'Time', 'Dropped packets (%)')} />
              </ChartBox>
            )}
            {view === 'histogram' && <Histogram dir={stats.dir} />}
          </Col>
        </Row>
      </Card.Body>
    </Card>
  );
}
