// User flows of the Interfaces page.
import { screen, within } from '@testing-library/react';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

const rowOf = (pci: string) => must(screen.getByText(pci).closest('tr'), `the row of ${pci}`);

describe('Interfaces page', () => {
  it('lists every interface with its data, the usable flag and the reason when it is not usable', async () => {
    renderApp('/interfaces');
    await screen.findByText('0000:65:00.0');
    const usable = within(rowOf('0000:65:00.0'));
    for (const text of ['Mellanox', 'mlx5_core', 'enp101s0f0', 'aa:bb:cc:00:00:01', 'up', '100 Gb/s', '1500', 'yes']) {
      expect(usable.getByText(text)).toBeInTheDocument();
    }
    const unusable = within(rowOf('0000:b3:00.0'));
    expect(unusable.getByText('no')).toBeInTheDocument();
    expect(unusable.getByText('driver i40e: bind to vfio-pci')).toBeInTheDocument();
  });

  it('shows n/a for missing data, and says "uplink" for an uplink that comes without a reason', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({
        pci: '0000:17:00.0',
        vendor: null,
        driver: null,
        speed_mbps: null,
        mtu: null,
        pcie_link: null,
        uplink: true,
        dpds_usable: false,
        reason: null,
      }),
    ]);
    renderApp('/interfaces');
    await screen.findByText('0000:17:00.0');
    const row = within(rowOf('0000:17:00.0'));
    expect(row.getAllByText('n/a').length).toBeGreaterThanOrEqual(3);
    expect(row.getByText('uplink')).toBeInTheDocument();
    // the hex vendor id stands in for a vendor whose name is unknown
    expect(row.getByText('0x15b3')).toBeInTheDocument();
  });

  it('says so when there are no interfaces, and shows the API error', async () => {
    api.getInterfaces.mockResolvedValueOnce([]);
    const view = renderApp('/interfaces');
    expect(await screen.findByText('No network interfaces found.')).toBeInTheDocument();
    view.unmount();

    api.getInterfaces.mockRejectedValue(new Error('no sysfs'));
    renderApp('/interfaces');
    expect(await screen.findByRole('alert')).toHaveTextContent('no sysfs');
  });
});
