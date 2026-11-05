import { screen, waitFor, within } from '@testing-library/react';
import { vi } from 'vitest';
import * as fx from '../test/fixtures';
import { renderApp, tick } from '../test/helpers';
import { api, ApiError } from '../test/mockApi';
import { must } from '../test/must';

// Spec 35 F6 (user 2026-11-05): the NIC ports table is off the dashboard and on the Interfaces page, "same content": heading
// "NIC ports", data-testid="ports-table", columns ipackets ... rx_nombuf, first column the netdev name (device name as the
// fallback, "<n> (unnamed)" without a name), striped with a dark header. The data are the `ports` of `GET /api/stats`, which the
// page polls like the dashboard did (about once a second). These tests are the ones that used to live in dashboard.test.tsx
// ("NIC ports table"), dashboardP4tg.test.tsx ("PortsTable style") and the port-label block of dashboard.test.tsx.
// CHOICE: a failing stats request leaves the interface list alone and shows no alert (the table is simply not updated).

const COLUMNS = ['ipackets', 'opackets', 'ibytes', 'obytes', 'imissed', 'ierrors', 'oerrors', 'rx_nombuf'];

function cell(table: HTMLElement, rowText: string, column: string): string {
  const headers = within(table)
    .getAllByRole('columnheader')
    .map((h) => h.textContent?.trim());
  const col = headers.indexOf(column);
  if (col < 0) throw new Error(`no column ${column} in [${headers.join(', ')}]`);
  const row = within(table)
    .getAllByRole('row')
    .find((r) => r.children[0]?.textContent?.includes(rowText));
  if (!row) throw new Error(`no row for ${rowText}`);
  return (row.children[col]?.textContent ?? '').trim();
}
const digits = (t: string) => t.replace(/[\s,.  ]/g, '');
const portNames = (table: HTMLElement) =>
  within(table)
    .getAllByRole('rowheader')
    .map((h) => h.textContent?.trim());

const withPorts = (ports: ReturnType<typeof fx.portStats>[]) =>
  api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { ports }));

describe('Interfaces page (spec 35 F6): the NIC ports table', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
  });

  it('is below the interface list, which keeps its columns, and has a heading naming it', async () => {
    withPorts([fx.portStats(0)]);
    renderApp('/interfaces');
    await screen.findByTestId('ports-table');
    expect(screen.getByRole('heading', { name: /NIC ports/i })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'PCI' })).toBeInTheDocument();
    expect(screen.getByRole('columnheader', { name: 'Netdev' })).toBeInTheDocument();
  });

  it('has one row per port with every counter in its column', async () => {
    withPorts([
      fx.portStats(0),
      fx.portStats(1, {
        name: '0000:65:00.1',
        ipackets: 5,
        opackets: 6,
        ibytes: 7,
        obytes: 8,
        imissed: 9,
        ierrors: 10,
        oerrors: 12,
        rx_nombuf: 13,
      }),
    ]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    await within(table).findByRole('rowheader', { name: 'enp101s0f0' });
    const headers = within(table)
      .getAllByRole('columnheader')
      .map((h) => h.textContent?.trim());
    for (const c of COLUMNS) expect(headers).toContain(c);
    expect(within(table).getAllByRole('row')).toHaveLength(3); // header + 2 ports
    const expected0: Record<string, string> = {
      ipackets: '1234567',
      opackets: '7654321',
      ibytes: '98765432100',
      obytes: '12345678900',
      imissed: '11',
      ierrors: '22',
      oerrors: '33',
      rx_nombuf: '44',
    };
    for (const c of COLUMNS) expect(digits(cell(table, 'enp101s0f0', c))).toBe(expected0[c]);
    const expected1: Record<string, string> = {
      ipackets: '5',
      opackets: '6',
      ibytes: '7',
      obytes: '8',
      imissed: '9',
      ierrors: '10',
      oerrors: '12',
      rx_nombuf: '13',
    };
    for (const c of COLUMNS) expect(digits(cell(table, 'enp101s0f1', c))).toBe(expected1[c]);
  });

  it('shows zero counters as 0, not as n/a or empty', async () => {
    withPorts([fx.portStats(0, { imissed: 0, ierrors: 0, oerrors: 0, rx_nombuf: 0 })]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    await within(table).findByRole('rowheader', { name: 'enp101s0f0' });
    for (const c of ['imissed', 'ierrors', 'oerrors', 'rx_nombuf']) expect(cell(table, 'enp101s0f0', c)).toBe('0');
  });

  it('falls back to the port number when the port has no name', async () => {
    withPorts([fx.portStats(7, { name: null })]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    const rows = within(table).getAllByRole('row');
    expect(rows).toHaveLength(2);
    expect(rows[1].textContent).toMatch(/\b7\b/);
  });

  it('is not rendered without ports (the stats of a stopped emulation carry none)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { ports: [], stale: true, age_ms: undefined }));
    renderApp('/interfaces');
    await screen.findByRole('cell', { name: '0000:65:00.0' });
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /NIC ports/i })).not.toBeInTheDocument();
  });

  it('is a striped table with a dark header', async () => {
    withPorts([fx.portStats(0), fx.portStats(1)]);
    renderApp('/interfaces');
    const wrap = await screen.findByTestId('ports-table');
    const table = must(wrap.querySelector('table'), 'the table');
    expect(table).toHaveClass('table-striped');
    expect(must(table.querySelector('thead'), 'thead')).toHaveClass('table-dark');
  });

  it('the first column shows the netdev only', async () => {
    withPorts([fx.portStats(0), fx.portStats(1, { name: '0000:65:00.1' })]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    await waitFor(() => expect(portNames(table)).toEqual(['enp101s0f0', 'enp101s0f1']));
  });

  it('a vdev port, a port without netdev and an unknown PCI address keep their plain name', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({ pci: '0000:65:00.0', netdev: null }),
      fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' }),
    ]);
    withPorts([
      fx.portStats(0), // vfio-bound, no netdev
      fx.portStats(1, { name: 'net_null0' }), // vdev
      fx.portStats(2, { name: '0000:b3:00.0' }), // not in the list
      fx.portStats(3, { name: '0000:65:00.1' }), // with a netdev, so that the labels have loaded
    ]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    await waitFor(() => expect(portNames(table)).toContain('enp101s0f1'));
    expect(portNames(table)).toEqual(['0000:65:00.0', 'net_null0', '0000:b3:00.0', 'enp101s0f1']);
  });

  it('with a failing interface fetch the names are the plain device names and the table still comes', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    withPorts([fx.portStats(0), fx.portStats(7, { name: null })]);
    renderApp('/interfaces');
    const table = await screen.findByTestId('ports-table');
    expect(portNames(table)[0]).toBe('0000:65:00.0');
    expect(within(table).getAllByRole('row')[2].textContent).toMatch(/\b7\b/);
    errors.mockRestore();
  });

  it('follows the next polls', async () => {
    vi.useFakeTimers();
    withPorts([fx.portStats(0, { ipackets: 100 })]);
    renderApp('/interfaces');
    await tick(0);
    await tick(0);
    const table = screen.getByTestId('ports-table');
    expect(digits(cell(table, 'enp101s0f0', 'ipackets'))).toBe('100');
    withPorts([fx.portStats(0, { ipackets: 250 })]);
    await tick(2500);
    expect(digits(cell(screen.getByTestId('ports-table'), 'enp101s0f0', 'ipackets'))).toBe('250');
  });

  it('polls about once a second and stops after the page is left', async () => {
    vi.useFakeTimers();
    withPorts([fx.portStats(0)]);
    const view = renderApp('/interfaces');
    await tick(0);
    await tick(0);
    const n = api.getStats.mock.calls.length;
    await tick(5000);
    expect(api.getStats.mock.calls.length - n).toBeGreaterThanOrEqual(3);
    view.unmount();
    const after = api.getStats.mock.calls.length;
    await tick(20000);
    expect(api.getStats.mock.calls.length).toBe(after);
  });

  it('never has two stats requests in flight (a request takes 3.5 s)', async () => {
    vi.useFakeTimers();
    let active = 0;
    let max = 0;
    api.getStats.mockImplementation(async () => {
      active += 1;
      max = Math.max(max, active);
      await new Promise((r) => setTimeout(r, 3500));
      active -= 1;
      return fx.stats([fx.dirStats(0)], { ports: [fx.portStats(0)] });
    });
    renderApp('/interfaces');
    await tick(0);
    for (let i = 0; i < 20; i += 1) await tick(1000);
    expect(max).toBe(1);
  });

  it('a failing stats request leaves the interface list alone and shows no alert', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getStats.mockRejectedValue(new ApiError(502, 'controller unreachable'));
    renderApp('/interfaces');
    expect(await screen.findByRole('cell', { name: '0000:65:00.0' })).toBeInTheDocument();
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
    errors.mockRestore();
  });

  it('a stale payload without ports after a stop removes the table again', async () => {
    vi.useFakeTimers();
    withPorts([fx.portStats(0)]);
    renderApp('/interfaces');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('ports-table')).toBeInTheDocument();
    api.getStats.mockResolvedValue(fx.stats([], { ports: [], stale: true, age_ms: undefined }));
    await tick(2500);
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
  });
});
