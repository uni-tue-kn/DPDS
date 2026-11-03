import { screen, within } from '@testing-library/react';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

// Groups are <section> regions named by an <h2> starting with Errors / Warnings / Info / OK (an optional
// count may follow), in this order, empty groups omitted. Each group has `data-level` (error|warn|info|ok);
// each hint is a listitem with the same `data-level` showing title, detail and suggestion. The level
// colours come from CSS variables keyed on `data-level` (not testable in jsdom; see assets/styles.test.ts).
describe('Host hints page', () => {
  it('groups hints by level in severity order', async () => {
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    const headings = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent ?? '');
    expect(headings).toHaveLength(4);
    expect(headings[0]).toMatch(/^Errors/);
    expect(headings[1]).toMatch(/^Warnings/);
    expect(headings[2]).toMatch(/^Info/);
    expect(headings[3]).toMatch(/^OK/);
  });

  it('puts each hint into its group with title, detail and suggestion', async () => {
    renderApp('/hints');
    const errors = await screen.findByRole('region', { name: /^Errors/ });
    expect(within(errors).getAllByRole('listitem')).toHaveLength(1);
    expect(errors).toHaveTextContent('No core isolation');
    expect(errors).toHaveTextContent('isolcpus is not set.');
    expect(errors).toHaveTextContent('Add isolcpus=2-5 nohz_full=2-5 rcu_nocbs=2-5 to the kernel command line.');
    const warns = screen.getByRole('region', { name: /^Warnings/ });
    expect(within(warns).getAllByRole('listitem')).toHaveLength(2);
    expect(warns).toHaveTextContent('Few hugepages');
    expect(warns).toHaveTextContent('Governor is powersave');
    expect(within(screen.getByRole('region', { name: /^Info/ })).getAllByRole('listitem')).toHaveLength(1);
    expect(within(screen.getByRole('region', { name: /^OK/ })).getAllByRole('listitem')).toHaveLength(1);
    expect(errors).not.toHaveTextContent('Few hugepages');
  });

  it('marks groups and items with their level for the colours', async () => {
    renderApp('/hints');
    const levels: [RegExp, string][] = [[/^Errors/, 'error'], [/^Warnings/, 'warn'], [/^Info/, 'info'], [/^OK/, 'ok']];
    for (const [name, level] of levels) {
      const region = await screen.findByRole('region', { name });
      expect(region).toHaveAttribute('data-level', level);
      for (const item of within(region).getAllByRole('listitem')) expect(item).toHaveAttribute('data-level', level);
    }
  });

  it('omits empty groups', async () => {
    api.getHostHints.mockResolvedValue(fx.hints().filter((h) => h.level === 'warn' || h.level === 'ok'));
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Warnings/ });
    expect(screen.queryByRole('region', { name: /^Errors/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: /^Info/ })).not.toBeInTheDocument();
    expect(screen.getByRole('region', { name: /^OK/ })).toBeInTheDocument();
  });

  it('shows the management NIC error hint in the Errors group', async () => {
    api.getHostHints.mockResolvedValue([
      { id: 'management_nic', level: 'error', title: 'Management NIC 0000:17:00.0', detail: 'carries a route', suggestion: 'Do not use it.' },
    ]);
    renderApp('/hints');
    const errors = await screen.findByRole('region', { name: /^Errors/ });
    expect(errors).toHaveTextContent('Management NIC 0000:17:00.0');
  });

  it('shows a message for an empty hint list', async () => {
    api.getHostHints.mockResolvedValue([]);
    renderApp('/hints');
    expect(await screen.findByText(/no hints/i)).toBeInTheDocument();
  });

  it('shows the API error', async () => {
    api.getHostHints.mockRejectedValue(new ApiError(500, 'procfs unreadable'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('procfs unreadable');
  });

  // v12: the page shows no ports of its own; port names in hint texts come from the controller and are printed verbatim.
  it('prints a port in a hint text verbatim and does not ask for the interfaces', async () => {
    api.getHostHints.mockResolvedValue([
      { id: 'numa', level: 'warn', title: 'NUMA mismatch', detail: 'Port 0000:65:00.0 is on node 1.', suggestion: 'Use cores of node 1.' },
    ]);
    renderApp('/hints');
    expect(await screen.findByText('Port 0000:65:00.0 is on node 1.')).toBeInTheDocument();
    expect(api.getInterfaces).not.toHaveBeenCalled();
  });
});
