import { screen, within } from '@testing-library/react';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

// Columns (columnheader names, in this order): PCI, Vendor, Driver, Netdev, MAC, State, Speed, MTU, NUMA,
// PCIe link, Usable, Reason. Null values print "n/a". Speed 100000 Mb/s prints "100 Gb/s", 10000 "10 Gb/s".
// kernel_route interfaces carry the text "management" in their row; no other row does.
const COLUMNS = [
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
];

function rowOf(pci: string): HTMLElement {
  return screen.getByRole('cell', { name: pci }).closest('tr') as HTMLElement;
}
function cell(row: HTMLElement, column: string): HTMLElement {
  const headers = screen.getAllByRole('columnheader').map((h) => h.textContent?.trim());
  const idx = headers.indexOf(column);
  if (idx < 0) throw new Error(`no column ${column}`);
  return within(row).getAllByRole('cell')[idx];
}

describe('Interfaces page', () => {
  it('has the discovery columns', async () => {
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:65:00.0' });
    expect(screen.getAllByRole('columnheader').map((h) => h.textContent?.trim())).toEqual(COLUMNS);
  });

  it('shows one row per interface with all fields', async () => {
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:65:00.0' });
    expect(screen.getAllByRole('row')).toHaveLength(1 + 4);
    const r = rowOf('0000:65:00.0');
    expect(cell(r, 'Vendor')).toHaveTextContent('Mellanox');
    expect(cell(r, 'Driver')).toHaveTextContent('mlx5_core');
    expect(cell(r, 'Netdev')).toHaveTextContent('enp101s0f0');
    expect(cell(r, 'MAC')).toHaveTextContent('aa:bb:cc:00:00:01');
    expect(cell(r, 'State')).toHaveTextContent('up');
    expect(cell(r, 'Speed')).toHaveTextContent('100 Gb/s');
    expect(cell(r, 'MTU')).toHaveTextContent('1500');
    expect(cell(r, 'NUMA')).toHaveTextContent('0');
    expect(cell(r, 'PCIe link').textContent).toMatch(/16\.0 GT\/s.*x16/);
    expect(cell(r, 'Usable')).toHaveTextContent(/yes/i);
    expect(r).not.toHaveTextContent(/management/i);
  });

  it('shows why an interface is unusable', async () => {
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:b3:00.0' });
    const r = rowOf('0000:b3:00.0');
    expect(cell(r, 'Vendor')).toHaveTextContent('Intel');
    expect(cell(r, 'Usable')).toHaveTextContent(/no/i);
    expect(cell(r, 'Reason')).toHaveTextContent('driver i40e: bind to vfio-pci');
    expect(cell(r, 'Speed')).toHaveTextContent('10 Gb/s');
    expect(cell(r, 'PCIe link')).toHaveTextContent('n/a');
    expect(r).not.toHaveTextContent(/management/i);
  });

  it('flags the management NIC and prints n/a for missing sysfs data', async () => {
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:17:00.0' });
    const r = rowOf('0000:17:00.0');
    expect(r).toHaveTextContent(/management/i);
    expect(cell(r, 'Usable')).toHaveTextContent(/no/i);
    expect(cell(r, 'Speed')).toHaveTextContent('n/a');
    expect(cell(r, 'MTU')).toHaveTextContent('n/a');
    expect(cell(r, 'NUMA')).toHaveTextContent('n/a');
  });

  it('falls back to the hex vendor id when the vendor name is unknown', async () => {
    api.getInterfaces.mockResolvedValue([fx.iface({ vendor: null, vendor_id: '0x1af4' })]);
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:65:00.0' });
    expect(cell(rowOf('0000:65:00.0'), 'Vendor')).toHaveTextContent('0x1af4');
  });

  it('shows an empty state', async () => {
    api.getInterfaces.mockResolvedValue([]);
    renderApp('/interfaces');
    expect(await screen.findByText(/no network interfaces/i)).toBeInTheDocument();
  });

  it('shows the API error', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/interfaces');
    expect(await screen.findByRole('alert')).toHaveTextContent('sysfs unreadable');
  });
});
