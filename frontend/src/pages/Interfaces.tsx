import { Badge, Table } from 'react-bootstrap';
import * as api from '../api/client';
import ErrorAlert from '../components/ErrorAlert';
import { formatInt, formatSpeedMbps } from '../common/format';
import { useLoad } from '../common/useLoad';
import type { Interface } from '../api/types';

const NA = 'n/a';

function pcie(i: Interface): string {
  const l = i.pcie_link;
  if (!l || (l.speed == null && l.width == null)) return NA;
  return `${l.speed ?? NA} x${l.width ?? '?'}`;
}

function Row({ i }: { i: Interface }) {
  const reason = i.reason ?? (i.kernel_route ? 'management NIC' : '');
  return (
    <tr>
      <td>{i.pci}</td>
      <td>{i.vendor ?? i.vendor_id ?? NA}</td>
      <td>{i.driver ?? NA}</td>
      <td>{i.netdev ?? NA}</td>
      <td>{i.mac ?? NA}</td>
      <td>{i.operstate ?? NA}</td>
      <td>{formatSpeedMbps(i.speed_mbps)}</td>
      <td>{formatInt(i.mtu)}</td>
      <td>{formatInt(i.numa_node)}</td>
      <td>{pcie(i)}</td>
      <td>
        <Badge pill bg={i.dpds_usable ? 'success' : 'danger'}>
          {i.dpds_usable ? 'yes' : 'no'}
        </Badge>
      </td>
      <td>{reason}</td>
    </tr>
  );
}

export default function Interfaces() {
  const { data, error } = useLoad(api.getInterfaces);
  return (
    <>
      <h1 className="mb-3">Interfaces</h1>
      <ErrorAlert message={error} />
      {data && data.length === 0 && <p>No network interfaces found.</p>}
      {data && data.length > 0 && (
        <Table striped size="sm" responsive>
          <thead className="table-dark">
            <tr>
              {[
                'PCI',
                'Vendor',
                'Driver',
                'Netdev',
                'MAC',
                'State',
                'Speed',
                'MTU',
                'NUMA',
                'PCIe link',
                'Usable',
                'Reason',
              ].map((h) => (
                <th key={h}>{h}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {data.map((i) => (
              <Row key={i.pci} i={i} />
            ))}
          </tbody>
        </Table>
      )}
    </>
  );
}
