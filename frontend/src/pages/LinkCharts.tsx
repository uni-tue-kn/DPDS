import { Fragment, useId, useMemo, useState } from 'react';
import { Form, OverlayTrigger, Tooltip } from 'react-bootstrap';
import { Line } from 'react-chartjs-2';
import type { HistorySample } from '../api/types';
import { formatElapsed } from '../common/format';
import { useTheme } from '../common/theme';
import { chartOptions, RX_COLOR, RX_FILL, TX_COLOR, TX_FILL } from '../components/charts';
import ChartBox from './ChartBox';
import DelayHistogram from './DelayHistogram';
import type { DropCounts } from './LossDonut';
import LossDonut from './LossDonut';

type View = 'rate' | 'histogram' | 'loss';

const MODEL_COLOR = '#27ae60';
const OVERLOAD_COLOR = '#e74c3c';

/**
 * The chart area of one link: the chart, the radios below it and the loss donut. `live` carries the live delay numbers; a
 * link of an ended run has none, and so no histogram (it needs live telemetry), nor has a link with delay statistics off.
 * Without `dir` it is the Summary of all links: `history` and `drops` are their aggregates, and the histogram radio is
 * there but disabled (a histogram is per link).
 */
export default function LinkCharts({
  dir,
  dirLabel = '',
  active,
  history,
  drops,
  live,
}: {
  dir?: number;
  /** "ingress → egress" of the link, for the histogram tables. */
  dirLabel?: string;
  /** Whether the link's tab is shown: react-bootstrap keeps hidden panes mounted, and a hidden histogram must not poll. */
  active: boolean;
  history: HistorySample[];
  drops: DropCounts;
  live?: { delayStats: boolean; meanNs?: number | null; stdNs?: number | null };
}) {
  const theme = useTheme();
  const [chosen, setChosen] = useState<View>('rate');
  const id = useId();
  const summary = dir === undefined;
  const of = summary ? 'all links' : `link ${dir}`;
  const hasHistogram = live?.delayStats === true;
  const view = chosen === 'histogram' && !hasHistogram ? 'rate' : chosen;
  const views: [View, string][] = [
    ['rate', 'Rate'],
    ...(hasHistogram || summary ? ([['histogram', 'Delay histogram']] as [View, string][]) : []),
    ['loss', 'Loss'],
  ];
  const labels = useMemo(() => history.map((s) => formatElapsed(s.t_ms)), [history]);
  const rateData = useMemo(
    () => ({
      labels,
      datasets: [
        {
          label: 'RX',
          data: history.map((s) => s.rx_gbps),
          borderColor: RX_COLOR,
          backgroundColor: RX_FILL,
          fill: true,
        },
        {
          label: 'TX',
          data: history.map((s) => s.tx_gbps),
          borderColor: TX_COLOR,
          backgroundColor: TX_FILL,
          fill: true,
        },
      ],
    }),
    [history, labels],
  );
  // The loss model is intended in an emulator; everything else is the emulator running out of room.
  const lossData = useMemo(
    () => ({
      labels,
      datasets: [
        {
          label: 'Model loss %',
          data: history.map((s) => s.loss_pct),
          borderColor: MODEL_COLOR,
          backgroundColor: MODEL_COLOR,
        },
        {
          label: 'Overload drops %',
          data: history.map((s) => s.limiter_pct + s.buffer_full_pct + s.nic_tx_pct + s.nic_rx_pct),
          borderColor: OVERLOAD_COLOR,
          backgroundColor: OVERLOAD_COLOR,
        },
      ],
    }),
    [history, labels],
  );
  const rateOptions = useMemo(() => chartOptions<'line'>(theme, 'Time', 'Rate (Gbit/s)'), [theme]);
  const lossOptions = useMemo(() => chartOptions<'line'>(theme, 'Time', 'Dropped packets (%)'), [theme]);

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
        <DelayHistogram dir={dir} dirLabel={dirLabel} meanNs={live?.meanNs} stdNs={live?.stdNs} />
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
              disabled={summary && v === 'histogram'}
              onChange={() => setChosen(v)}
            />
          );
          return summary && v === 'histogram' ? (
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
      <LossDonut drops={drops} />
    </>
  );
}
