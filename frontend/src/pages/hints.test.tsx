import { screen, waitFor, within } from '@testing-library/react';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { expectStartBody, renderApp, seedStore, seedLinks } from '../test/helpers';

// v22: the page lists the host-only hints (`getHostHints`, GET /api/host/hints) plus, when the LOCAL document has links, the
// link-dependent ones from `getHostHintsFor({links, settings})` (POST /api/host/hints with the start body); merged by hint id,
// the POST answer wins. No local links -> no POST.
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
    const levels: [RegExp, string][] = [
      [/^Errors/, 'error'],
      [/^Warnings/, 'warn'],
      [/^Info/, 'info'],
      [/^OK/, 'ok'],
    ];
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

  it('shows the host uplink error hint in the Errors group', async () => {
    api.getHostHints.mockResolvedValue([
      {
        id: 'uplink',
        level: 'error',
        title: 'Host uplink',
        detail: 'carries a route',
        suggestion: 'Do not use it.',
      },
    ]);
    renderApp('/hints');
    const errors = await screen.findByRole('region', { name: /^Errors/ });
    expect(errors).toHaveTextContent('Host uplink');
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
      {
        id: 'numa',
        level: 'warn',
        title: 'NUMA mismatch',
        detail: 'Port 0000:65:00.0 is on node 1.',
        suggestion: 'Use cores of node 1.',
      },
    ]);
    renderApp('/hints');
    expect(await screen.findByText('Port 0000:65:00.0 is on node 1.')).toBeInTheDocument();
    expect(api.getInterfaces).not.toHaveBeenCalled();
  });
});

describe('Host hints page: hints for the local configuration (v22)', () => {
  const linkHint = {
    id: 'numa',
    level: 'warn' as const,
    title: 'NUMA mismatch',
    detail: 'a-b crosses nodes.',
    suggestion: 'Use node 0.',
  };

  it('without local links only the host hints are asked for: no POST', async () => {
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    expect(api.getHostHints).toHaveBeenCalledTimes(1);
    expect(api.getHostHintsFor).not.toHaveBeenCalled();
  });

  it('with local links it POSTs the start body (links without ids, settings) and lists the host and the link hints together', async () => {
    const doc = seedStore({
      links: [fx.link(), fx.linkRev()],
      settings: fx.settings({ main_lcore: 2, poll_interval_ms: 250 }),
    });
    api.getHostHintsFor.mockResolvedValue([linkHint]);
    renderApp('/hints');
    const warns = await screen.findByRole('region', { name: /^Warnings/ });
    await waitFor(() => expect(warns).toHaveTextContent('NUMA mismatch'));
    expect(warns).toHaveTextContent('Few hugepages'); // host hint of the GET
    expect(api.getHostHints).toHaveBeenCalledTimes(1);
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(1);
    expectStartBody(api.getHostHintsFor.mock.calls[0][0], doc);
    expect(within(warns).getAllByRole('listitem')).toHaveLength(3);
  });

  it('a hint id in both answers is listed once and the POST answer wins', async () => {
    seedLinks([fx.link()]);
    api.getHostHintsFor.mockResolvedValue([
      {
        id: 'hugepages',
        level: 'error',
        title: 'Hugepages too few for the links',
        detail: 'a-b needs 8 GiB.',
        suggestion: 'Reserve 8 GiB.',
      },
    ]);
    renderApp('/hints');
    const errors = await screen.findByRole('region', { name: /^Errors/ });
    await waitFor(() => expect(errors).toHaveTextContent('Hugepages too few for the links'));
    expect(screen.queryByText('Few hugepages')).not.toBeInTheDocument();
    expect(screen.getAllByText(/hugepages/i).length).toBeGreaterThan(0);
    expect(within(screen.getByRole('region', { name: /^Warnings/ })).getAllByRole('listitem')).toHaveLength(1); // governor only
  });

  it('a failing POST shows its message in an alert while the host hints stay', async () => {
    seedLinks([fx.link()]);
    api.getHostHintsFor.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a host uplink');
    expect(screen.getByRole('region', { name: /^Errors/ })).toHaveTextContent('No core isolation');
  });

  it('a failing GET still shows its error, also when there are links', async () => {
    seedLinks([fx.link()]);
    api.getHostHints.mockRejectedValue(new ApiError(500, 'procfs unreadable'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('procfs unreadable');
  });

  it('an empty answer of both calls shows the empty message', async () => {
    seedLinks([fx.link()]);
    api.getHostHints.mockResolvedValue([]);
    api.getHostHintsFor.mockResolvedValue([]);
    renderApp('/hints');
    expect(await screen.findByText(/no hints/i)).toBeInTheDocument();
  });

  it('reads the document but never writes it', async () => {
    seedLinks([fx.link()]);
    const before = localStorage.getItem('dpds.config.v1');
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
  });
});
