import { useId } from 'react';
import { OverlayTrigger, Row, Table, Tooltip } from 'react-bootstrap';
import { formatBits, formatFrameCount } from '../common/format';

/** The live rates of a link, or of all links added up. */
export interface Rates {
  txBps: number;
  txPps: number;
  rxBps: number;
  rxPps: number;
}

/** The run's frame counts of a link, or of all links added up. */
export interface FrameTotals {
  /** The frames the link forwarded. */
  forwarded: number;
  outOfOrder: number;
}

/**
 * Preamble and inter-frame gap per frame, in bits: P4TG's L1 is L2 plus 20 B per frame. Whether DPDK's byte counters include the
 * 4 B FCS is unverified; it is not added here.
 */
const L1_OVERHEAD_BITS = 20 * 8;

function CountCell({ count }: { count: number }) {
  const id = useId();
  return (
    <td>
      <OverlayTrigger placement="top" overlay={<Tooltip id={id}>{`${count}`}</Tooltip>}>
        <span>{formatFrameCount(count)}</span>
      </OverlayTrigger>
    </td>
  );
}

/** P4TG's rate table (StatView.tsx): TX and RX at layer 1 and layer 2. */
export function RateTable({ rates }: { rates: Rates }) {
  const { txBps, txPps, rxBps, rxPps } = rates;
  return (
    <Row className="mb-3">
      <div className="col-12 col-md-6 col-sm-12">
        <Table striped bordered hover size="sm" className="mt-3 mb-3">
          <thead className="table-dark">
            <tr>
              <th className="col-3">TX L1</th>
              <th className="col-3">RX L1</th>
              <th className="col-3">TX L2</th>
              <th className="col-3">RX L2</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <td>{formatBits(txBps + L1_OVERHEAD_BITS * txPps)}</td>
              <td>{formatBits(rxBps + L1_OVERHEAD_BITS * rxPps)}</td>
              <td>{formatBits(txBps)}</td>
              <td>{formatBits(rxBps)}</td>
            </tr>
          </tbody>
        </Table>
      </div>
    </Row>
  );
}

/** P4TG's loss table (StatView.tsx): lost frames, their share of all frames, and the out-of-order departures. */
export function LossTable({ lost, frames }: { lost: number; frames: FrameTotals }) {
  const ratio = lost > 0 ? `${((lost * 100) / (lost + frames.forwarded)).toFixed(2)} %` : '0.00 %';
  return (
    <Row>
      <div className="col-12 col-sm-12 col-md-4">
        <Table striped bordered hover size="sm" className="mt-3 mb-3">
          <thead className="table-dark">
            <tr>
              <th>Lost Frames</th>
              <th>Frame Loss Ratio</th>
              <th>Out of Order</th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <CountCell count={lost} />
              <td>{ratio}</td>
              <CountCell count={frames.outOfOrder} />
            </tr>
          </tbody>
        </Table>
      </div>
    </Row>
  );
}
