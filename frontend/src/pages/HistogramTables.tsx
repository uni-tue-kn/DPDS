import { useId } from 'react';
import { Col, OverlayTrigger, Row, Table, Tooltip } from 'react-bootstrap';
import type { HistogramView } from '../api/types';
import { formatCount, formatDuration } from '../common/format';

const NA = '-';

/** A duration, or "-" when there is none. */
const duration = (ns?: number | null) => (ns == null ? NA : formatDuration(ns));

/** A table: the header row shows `tip` on hover, one body row follows. */
function StatTable({
  tip,
  heads,
  cells,
  overlined,
}: {
  tip: string;
  heads: string[];
  cells: string[];
  /** The header drawn with an overline (a mean). */
  overlined?: string;
}) {
  const id = useId();
  return (
    <Col xs={12} md={4}>
      <Table striped bordered hover size="sm" className="mt-3 mb-3">
        <OverlayTrigger placement="top" overlay={<Tooltip id={id}>{tip}</Tooltip>}>
          <thead className="table-dark">
            <tr>
              {heads.map((h) => (
                <th key={h}>{h === overlined ? <span style={{ textDecoration: 'overline' }}>{h}</span> : h}</th>
              ))}
            </tr>
          </thead>
        </OverlayTrigger>
        <tbody>
          <tr>
            {cells.map((c, i) => (
              <td key={heads[i]}>{c}</td>
            ))}
          </tr>
        </tbody>
      </Table>
    </Col>
  );
}

/** The numbers of a link's delay (or, with `name`, IAT) histogram in rows. `meanNs` and `stdNs` are its mean and σ. */
export default function HistogramTables({
  hist,
  name = 'Delay',
  meanNs,
  stdNs,
}: {
  hist: HistogramView;
  name?: 'Delay' | 'IAT';
  meanNs?: number | null;
  stdNs?: number | null;
}) {
  const lo = hist.lo_ns;
  const outliers = hist.underflow + hist.overflow;
  const total = hist.count ?? hist.bins.reduce((a, b) => a + b, 0) + outliers;
  const p = hist.percentiles;
  return (
    <Row>
      <StatTable
        tip="Histogram config"
        heads={['Bin Range', '#Bins', 'Bin Width']}
        cells={[
          `[${formatDuration(lo)}, ${formatDuration(lo + hist.bins.length * hist.bin_ns)}]`,
          formatCount(hist.bins.length),
          formatDuration(hist.bin_ns),
        ]}
      />
      <StatTable
        tip="Values calculated from histogram data"
        heads={[name, `σ(${name})`, 'Outlier', 'Total #Packets']}
        overlined={name}
        cells={[duration(meanNs), duration(stdNs), formatCount(outliers), formatCount(total)]}
      />
      <StatTable
        tip="Percentiles"
        heads={['p50', 'p90', 'p99', 'p99.9']}
        cells={[duration(p?.p50), duration(p?.p90), duration(p?.p99), duration(p?.p999)]}
      />
    </Row>
  );
}
