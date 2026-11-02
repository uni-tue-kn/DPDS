import { Card, Table } from 'react-bootstrap';
import { formatCount } from '../common/format';
import type { PortStats } from '../api/types';

const COLUMNS = ['ipackets', 'opackets', 'ibytes', 'obytes', 'imissed', 'ierrors', 'oerrors', 'rx_nombuf'] as const;

/** Raw NIC counters per port (`rte_eth_stats`). */
export default function PortsTable({ ports }: { ports: PortStats[] }) {
  return (
    <Card className="mb-3" data-testid="ports-table">
      <Card.Header as="h2" className="h6 mb-0">NIC ports</Card.Header>
      <Table responsive size="sm" className="mb-0">
        <thead>
          <tr>
            <th scope="col">Port</th>
            {COLUMNS.map((c) => <th key={c} scope="col">{c}</th>)}
          </tr>
        </thead>
        <tbody>
          {ports.map((p) => (
            <tr key={p.port}>
              <th scope="row">{p.name ?? `${p.port} (unnamed)`}</th>
              {COLUMNS.map((c) => <td key={c}>{formatCount(p[c])}</td>)}
            </tr>
          ))}
        </tbody>
      </Table>
    </Card>
  );
}
