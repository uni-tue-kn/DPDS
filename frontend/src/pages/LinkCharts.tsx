import { Fragment, useId, useMemo, useState } from 'react';
import { Form, OverlayTrigger, Row, Tooltip } from 'react-bootstrap';
import { Line } from 'react-chartjs-2';
import type { HistorySample } from '../api/types';
import { secondsToTime } from '../common/format';
import { selectTimeKeys } from '../common/selectTimeKeys';
import { useTheme } from '../common/theme';
import { chartOptions, RX_COLOR, RX_FILL, TX_COLOR, TX_FILL } from '../components/charts';
import ChartBox from './ChartBox';
import DelayHistogram from './DelayHistogram';
import FrameDonut from './FrameDonut';
import { LossTable, RateTable } from './LinkTables';
import LossDonut, { CAUSES, totalDrops } from './LossDonut';
import type { LinkStats } from './linkStats';

type View = 'rate' | 'histogram' | 'iat' | 'loss';

/** The most points a time chart plots, like P4TG's `time_statistics?limit=100`. */
const MAX_POINTS = 100;

/** The units of the rate chart's y axis, largest first; with no traffic it is Gbit/s, like P4TG. */
const RATE_AXIS_UNITS = [
  { label: 'Gbit/s', perGbit: 1 },
  { label: 'Mbit/s', perGbit: 1e3 },
  { label: 'kbit/s', perGbit: 1e6 },
  { label: 'bit/s', perGbit: 1e9 },
] as const;

/** The cumulative count of each cause, in the order of the donut's `CAUSES`. */
const COUNTS = [
  (s: HistorySample) => s.loss_drops,
  (s: HistorySample) => s.limiter_drops,
  (s: HistorySample) => s.buffer_full_drops,
  (s: HistorySample) => s.nic_tx_drops,
  (s: HistorySample) => s.nic_rx_missed,
  (s: HistorySample) => s.nic_rx_nombuf,
  (s: HistorySample) => s.nic_rx_port_buffer_discards,
  (s: HistorySample) => s.nic_rx_phy_discards,
  (s: HistorySample) => s.nic_tx_phy_discards,
];

/**
 * The chart area of one link: the chart, the radios below it, P4TG's rate and loss tables and a row of the loss donut and the frame
 * size donut. `live` carries the live delay numbers; a
 * link of an ended run has none, but keeps its histogram (the controller answers the run's last one) when `endedStats` says
 * the run had delay statistics. A link with delay statistics off has no histogram (delay or IAT).
 * Without `dir` it is the Summary of all links: `history` and `stats` are their aggregates, and the histogram radio is
 * there but disabled (a histogram is per link), like the IAT one.
 */
export default function LinkCharts({
  dir,
  active,
  history,
  stats,
  live,
  empty = false,
  endedStats = false,
}: {
  dir?: number;
  /** Whether the link's tab is shown: react-bootstrap keeps hidden panes mounted, and a hidden histogram must not poll. */
  active: boolean;
  history: HistorySample[];
  stats: LinkStats;
  /** Before any run: no telemetry, so the histogram radio is there but disabled, like the Summary's, and there are no tables. */
  empty?: boolean;
  /** An ended run whose link had delay statistics: the histogram stays, from the controller's copy. */
  endedStats?: boolean;
  live?: { delayStats: boolean; meanNs?: number | null; stdNs?: number | null };
}) {
  const theme = useTheme();
  const [chosen, setChosen] = useState<View>('rate');
  const id = useId();
  // Which percentile lines are switched off, per histogram view: here so that they outlive the view and the tab.
  const [hiddenLines, setHiddenLines] = useState<Record<'histogram' | 'iat', ReadonlySet<string>>>({
    histogram: new Set(),
    iat: new Set(),
  });
  const toggleLine = (v: 'histogram' | 'iat') => (key: string) =>
    setHiddenLines((h) => {
      const next = new Set(h[v]);
      if (!next.delete(key)) next.add(key);
      return { ...h, [v]: next };
    });
  const summary = dir === undefined;
  const of = summary ? 'all links' : `link ${dir}`;
  const noHistogram = summary || empty;
  const hasHistogram = live?.delayStats === true || endedStats;
  const view = (chosen === 'histogram' || chosen === 'iat') && !hasHistogram ? 'rate' : chosen;
  const views: [View, string][] = [
    ['rate', 'Traffic rates'],
    ['loss', 'Packet loss'],
    ...(hasHistogram || noHistogram
      ? ([
          ['histogram', 'Delay histogram'],
          ['iat', 'IAT histogram'],
        ] as [View, string][])
      : []),
  ];
  // The history stays complete; the time charts plot at most MAX_POINTS of it.
  const points = useMemo(() => selectTimeKeys(history, MAX_POINTS), [history]);
  const labels = useMemo(() => points.map((s) => secondsToTime(Math.floor(s.t_ms / 1000))), [points]);
  // The y axis shows one unit, like P4TG's "Gbit/s"; a link slower than that gets the largest unit its peak reaches.
  const rateUnit = useMemo(() => {
    const peak = Math.max(0, ...points.map((s) => Math.max(s.rx_gbps, s.tx_gbps)));
    if (peak === 0) return RATE_AXIS_UNITS[0];
    return RATE_AXIS_UNITS.find((u) => peak * u.perGbit >= 1) ?? RATE_AXIS_UNITS[RATE_AXIS_UNITS.length - 1];
  }, [points]);
  const rateData = useMemo(
    () => ({
      labels,
      datasets: [
        {
          label: 'RX',
          data: points.map((s) => s.rx_gbps * rateUnit.perGbit),
          borderColor: RX_COLOR,
          backgroundColor: RX_FILL,
          fill: true,
        },
        {
          label: 'TX',
          data: points.map((s) => s.tx_gbps * rateUnit.perGbit),
          borderColor: TX_COLOR,
          backgroundColor: TX_FILL,
          fill: true,
        },
      ],
    }),
    [points, labels, rateUnit],
  );
  const lossData = useMemo(
    () => ({
      labels,
      datasets: CAUSES.map((c, i) => ({
        label: c.label,
        data: points.map(COUNTS[i]),
        borderColor: c.color,
        backgroundColor: c.color,
      })),
    }),
    [points, labels],
  );
  const rateOptions = useMemo(() => chartOptions<'line'>(theme, 'Time', rateUnit.label), [theme, rateUnit]);
  const lossOptions = useMemo(() => chartOptions<'line'>(theme, 'Time', 'Dropped packets'), [theme]);

  return (
    <>
      {view === 'rate' && (
        <ChartBox label={`Rate chart of ${of}`}>
          <Line data={rateData} options={rateOptions} />
        </ChartBox>
      )}
      {view === 'loss' && (
        <ChartBox label={`Loss chart of ${of}`}>
          <Line data={lossData} options={lossOptions} />
        </ChartBox>
      )}
      {view === 'histogram' && active && dir !== undefined && (
        <DelayHistogram
          dir={dir}
          meanNs={live?.meanNs}
          stdNs={live?.stdNs}
          hidden={hiddenLines.histogram}
          onToggle={toggleLine('histogram')}
        />
      )}
      {view === 'iat' && dir !== undefined && (
        <DelayHistogram dir={dir} active={active} kind="iat" hidden={hiddenLines.iat} onToggle={toggleLine('iat')} />
      )}
      <Form.Group role="radiogroup" aria-label="Chart" className="text-center my-3">
        {views.map(([v, text]) => {
          const radio = (
            <Form.Check
              key={v}
              inline
              type="radio"
              name={`chart-${id}`}
              id={`chart-${id}-${v}`}
              label={text}
              checked={view === v}
              disabled={noHistogram && (v === 'histogram' || v === 'iat')}
              onChange={() => setChosen(v)}
            />
          );
          return summary && (v === 'histogram' || v === 'iat') ? (
            <OverlayTrigger
              key={v}
              placement="top"
              overlay={<Tooltip id={`${id}-tip`}>Delay histograms are per link; select a link tab.</Tooltip>}
            >
              <span className="d-inline-block">{radio}</span>
            </OverlayTrigger>
          ) : (
            <Fragment key={v}>{radio}</Fragment>
          );
        })}
      </Form.Group>
      {!empty && (
        <Row className="mb-3">
          <RateTable rates={stats} />
          <LossTable lost={totalDrops(stats.drops)} frames={stats} />
        </Row>
      )}
      <Row>
        <div className="col-12 col-md-6 donut-divider">
          <LossDonut drops={stats.drops} />
        </div>
        <div className="col-12 col-md-6">
          <FrameDonut counts={stats.frameSizes} />
        </div>
      </Row>
    </>
  );
}
