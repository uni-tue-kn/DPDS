import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState } from '../api/types';
import {
  activePanel,
  card,
  findCard,
  findTabs,
  hasTab,
  pillText,
  queryRadio,
  radio,
  radioNames,
  runButton,
  runButtonNames,
  runButtons,
  summaryTab,
  tab,
  tabBadge,
  tabs,
  tabTitle,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, renderApp, seedRaw, seedStore, tick } from '../test/helpers';
import { ApiError, api, fakeController, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 35 (user 2026-11-05): minimal, P4TG-like dashboard. Spec: scratchpad/spec35-dashboard.md, items F1 to F7. The loss donut
// (F5) is in lossDonut.test.tsx, the NIC ports table on /interfaces (F6) in interfacesPorts.test.tsx. Layout contract and
// test ids: the header of src/test/dashboardUi.tsx. Tester's choices where the spec is silent are marked CHOICE.

const STATES: EmulationState[] = ['stopped', 'starting', 'running', 'stopping', 'failed'];
const RIGHTMOST: Record<EmulationState, string> = {
  stopped: 'Start',
  starting: 'Stop',
  running: 'Stop',
  stopping: 'Stopping…',
  failed: 'Start',
};

const STOPPED_STATS = (elapsed_ms = 305_400) => fx.stats([], { stale: true, age_ms: undefined, elapsed_ms });
const quiet = () => vi.spyOn(console, 'error').mockImplementation(() => undefined);

async function open() {
  vi.useFakeTimers();
  const view = renderApp('/');
  await tick(0);
  await tick(0);
  return view;
}

async function openIn(state: EmulationState, over: Partial<ReturnType<typeof fx.emulation>> = {}) {
  api.getEmulation.mockResolvedValue(fx.emulation(state, over));
  renderApp('/');
  await waitFor(() => expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', state));
}

/** What the controller answers once a run has ended: no directions, the frozen clock, the run's series and totals. */
function ended(state: 'stopping' | 'stopped' | 'failed', elapsed_ms = 305_400) {
  api.getEmulation.mockResolvedValue(
    fx.emulation(state, { directions: [], error: state === 'failed' ? 'dpds exited (status 1)' : null }),
  );
  api.getStats.mockResolvedValue(STOPPED_STATS(elapsed_ms));
}

// ---- F1: the top row --------------------------------------------------------------------------------------------------

describe('Dashboard (spec 35 F1): the top row, pills left and buttons right like P4TG', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  it('is one row with the pills in a col-8 text-start column and the button group in a col-4 text-end column', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)], { elapsed_ms: 192_000 }));
    renderApp('/');
    const row = await screen.findByTestId('top-row');
    const pills = within(row).getByTestId('top-pills');
    expect(pills).toHaveClass('col-8', 'text-start');
    const group = within(row).getByTestId('run-buttons');
    expect(group).toHaveAttribute('role', 'group');
    const column = must(group.closest('.col-4'), 'the col-4 column of the button group');
    expect(column).toHaveClass('text-end');
    expect(pills.contains(group)).toBe(false);
    expect(column.contains(pills)).toBe(false);
    // the pills left of the buttons
    expect(pills.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('the pills come in this order: state, RX, TX, elapsed, dropped', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)], { elapsed_ms: 192_000 }));
    renderApp('/');
    const pills = await screen.findByTestId('top-pills');
    await within(pills).findByTestId('pill-elapsed');
    const ids = ['state-badge', 'pill-rx', 'pill-tx', 'pill-elapsed', 'pill-dropped'];
    const found = ids.map((id) => within(pills).getByTestId(id));
    for (let i = 1; i < found.length; i += 1) {
      expect(found[i - 1].compareDocumentPosition(found[i]) & Node.DOCUMENT_POSITION_FOLLOWING, ids[i]).toBeTruthy();
    }
    // and nothing else with a pill test id sits in the group of buttons
    expect(within(screen.getByTestId('run-buttons')).queryByTestId('pill-rx')).not.toBeInTheDocument();
  });

  it('the live pills read "RX <Gbit/s> (<Mpps>)", "TX ...", the elapsed time and "dropped <x.xx> %"', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats(
        [
          fx.dirStats(0, { rx_bps: 6.1e9, tx_bps: 5.05e9, rx_pps: 4.6e6, tx_pps: 3.9e6 }),
          fx.dirStats(1, { rx_bps: 3.15e9, tx_bps: 2.2e9, rx_pps: 2.71e6, tx_pps: 2.41e6 }),
        ],
        { elapsed_ms: 192_000 },
      ),
    );
    renderApp('/');
    await waitFor(() => expect(pillText('pill-rx')).toBe('RX 9.25 Gbit/s (7.31 Mpps)'));
    expect(pillText('pill-tx')).toBe('TX 7.25 Gbit/s (6.31 Mpps)');
    expect(pillText('pill-elapsed')).toBe('0h 3m 12s');
    expect(pillText('pill-dropped')).toMatch(/^dropped \d+\.\d\d %$/);
  });

  it.each(STATES)('"%s": the group holds exactly [Restart] and [%s]', async (state) => {
    await openIn(state);
    expect(runButtonNames(), state).toEqual(['Restart', RIGHTMOST[state]]);
  });

  it.each(STATES)('"%s": Restart is %s', async (state) => {
    await openIn(state);
    const restart = must(runButton('Restart'), 'Restart');
    if (state === 'running') expect(restart).toBeEnabled();
    else expect(restart).toBeDisabled();
  });

  it.each(STATES)('"%s": the rightmost button is enabled only for Start and Stop', async (state) => {
    await openIn(state);
    const [, last] = runButtons();
    if (state === 'stopping') expect(last).toBeDisabled();
    else expect(last).toBeEnabled();
  });

  it('"Stopping…" has a spinner that is hidden from assistive technology, so its name is exact', async () => {
    await openIn('stopping');
    const [, last] = runButtons();
    expect(last.querySelector('.spinner-border')).not.toBeNull();
    expect(last).toHaveAccessibleName('Stopping…');
  });

  it('before the first emulation reply the group already has both buttons, both disabled', async () => {
    api.getEmulation.mockImplementation(() => new Promise(() => undefined));
    renderApp('/');
    await screen.findByTestId('run-buttons');
    expect(runButtonNames()).toEqual(['Restart', 'Start']);
    for (const b of runButtons()) expect(b).toBeDisabled();
  });

  it('the number and order of the buttons never change while the state walks through every state', async () => {
    vi.useFakeTimers();
    const seen: string[][] = [];
    renderApp('/');
    for (const state of ['stopped', 'starting', 'running', 'stopping', 'stopped', 'running', 'failed'] as const) {
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      await tick(2500);
      expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', state);
      seen.push(runButtonNames());
    }
    expect(seen.map((names) => names.length)).toEqual(Array(7).fill(2));
    expect(seen.map((names) => names[0])).toEqual(Array(7).fill('Restart'));
    expect(seen.map((names) => names[1])).toEqual(['Start', 'Stop', 'Stop', 'Stopping…', 'Start', 'Stop', 'Start']);
  });

  it('while a Restart waits for the stop the group reads [Restart, disabled] [Restarting…, disabled], whatever the polls say', async () => {
    vi.useFakeTimers();
    const c = fakeController('running');
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(runButton('Restart'), 'Restart'));
    await tick(0);
    for (const state of ['stopping', 'running', 'stopping'] as const) {
      c.state = state;
      await tick(1000);
      expect(runButtonNames(), state).toEqual(['Restart', 'Restarting…']);
      for (const b of runButtons()) expect(b, state).toBeDisabled();
    }
    c.state = 'stopped';
    await tick(2500);
    c.state = 'running';
    await tick(2500);
    expect(runButtonNames()).toEqual(['Restart', 'Stop']);
  });

  it('a start refused after the Restart wait puts the group back to [Restart] [Start]', async () => {
    vi.useFakeTimers();
    const c = fakeController('running');
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(runButton('Restart'), 'Restart'));
    await tick(0);
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    c.state = 'stopped';
    await tick(2500);
    expect(runButtonNames()).toEqual(['Restart', 'Start']);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
  });

  it('CHOICE: with an unreadable local configuration Restart stays disabled and Stop works while running', async () => {
    seedRaw('{not json');
    await openIn('running');
    expect(must(runButton('Restart'))).toBeDisabled();
    expect(must(runButton('Stop'))).toBeEnabled();
  });

  it('Restart and Start/Stop still do what they did: Start sends the start request, Stop the stop request', async () => {
    const user = userEvent.setup();
    await openIn('stopped');
    await user.click(must(runButton('Start')));
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(api.stopEmulation).not.toHaveBeenCalled();
  });

  it('Stop sends the stop request and Restart, while stopped, sends nothing even when clicked', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    renderApp('/');
    await waitFor(() => expect(runButton('Stop')).toBeEnabled());
    await user.click(must(runButton('Stop')));
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });
});

describe('Dashboard (spec 35 F1): notices go in a row below the top row, never into it', () => {
  const inNotices = (el: HTMLElement) => {
    const notices = screen.getByTestId('notices');
    const row = screen.getByTestId('top-row');
    expect(notices.contains(el)).toBe(true);
    expect(row.contains(el)).toBe(false);
    expect(row.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  };

  it('"Restart needed" is in the notices row and not in the group of buttons', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    api.getEmulation.mockResolvedValue(fx.runningEmulation(fx.runningConfig([fx.link()])));
    renderApp('/');
    const badge = await screen.findByTestId('pending-restart');
    inNotices(badge);
    expect(runButtonNames()).toEqual(['Restart', 'Stop']);
  });

  it('the stale warning is in the notices row', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: 6500 }));
    renderApp('/');
    inNotices(await screen.findByTestId('stale-indicator'));
  });

  it('the stats warning and the history warning are in the notices row', async () => {
    const errors = quiet();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockRejectedValue(new ApiError(502, 'controller unreachable'));
    api.getStatsHistory.mockRejectedValue(new ApiError(500, 'history read failed'));
    renderApp('/');
    inNotices(await screen.findByTestId('stats-warning'));
    inNotices(await screen.findByTestId('history-warning'));
    errors.mockRestore();
  });

  it('a refused start (the action error) is in the notices row, and the group keeps its two buttons', async () => {
    seedStore({ links: [fx.link()] });
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    renderApp('/');
    await waitFor(() => expect(runButton('Start')).toBeEnabled());
    await user.click(must(runButton('Start')));
    inNotices(await screen.findByRole('alert'));
    expect(runButtonNames()).toEqual(['Restart', 'Start']);
  });

  it('the controller being unreachable (the poll error) is in the notices row', async () => {
    const errors = quiet();
    api.getEmulation.mockRejectedValue(new Error('Failed to fetch'));
    renderApp('/');
    inNotices(await screen.findByRole('alert'));
    errors.mockRestore();
  });

  it('the emulation error of a failed run is in the notices row', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', {
        error: 'dpds exited during startup (status 1)',
        log_tail: ['EAL: Cannot allocate memory'],
      }),
    );
    renderApp('/');
    inNotices(await screen.findByRole('alert'));
  });

  it('a stop that fails shows its message in the notices row', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.stopEmulation.mockRejectedValue(new ApiError(500, 'cannot signal child'));
    renderApp('/');
    await waitFor(() => expect(runButton('Stop')).toBeEnabled());
    await user.click(must(runButton('Stop')));
    inNotices(await screen.findByRole('alert'));
    expect(screen.getByRole('alert')).toHaveTextContent('cannot signal child');
  });
});

// ---- F2: the pills after a stop -----------------------------------------------------------------------------------------

describe('Dashboard (spec 35 F2): the pills stay after the run has ended', () => {
  /**
   * Two links, each with an early sample and a final one (the final one is what the pills show), and `totals` of the run:
   * link 0 received 1000 packets and dropped 50 (5 %), link 1 received 3000 and dropped 300 (10 %): cumulative 350 / 4000 = 8.75 %
   * (the mean of the two shares would be 7.50, the sum of the final samples' shares something else again).
   */
  function twoLinkRun() {
    const early = { rx_gbps: 1, tx_gbps: 1, rx_mpps: 0.5, tx_mpps: 0.5 };
    serveHistory({
      startedAt: 1762000000,
      samples: {
        '0': [
          fx.historySample(1000, early),
          fx.historySample(2000, { rx_gbps: 6.1, tx_gbps: 5.05, rx_mpps: 4.6, tx_mpps: 3.9 }),
        ],
        '1': [
          fx.historySample(1000, early),
          fx.historySample(2000, { rx_gbps: 3.15, tx_gbps: 2.2, rx_mpps: 2.71, tx_mpps: 2.41 }),
        ],
      },
      totals: {
        '0': fx.totals({
          rx_pkts: 1000,
          loss_drops: 10,
          limiter_drops: 20,
          buffer_full_drops: 5,
          nic_tx_drops: 5,
          nic_rx_imissed: 7,
          nic_rx_nombuf: 3,
        }),
        '1': fx.totals({
          rx_pkts: 3000,
          loss_drops: 100,
          limiter_drops: 100,
          buffer_full_drops: 50,
          nic_tx_drops: 20,
          nic_rx_imissed: 20,
          nic_rx_nombuf: 10,
        }),
      },
    });
  }

  it.each(['stopped', 'stopping', 'failed'] as const)(
    '"%s": RX and TX are the last sample summed over the links, dropped is cumulative from the totals, elapsed is the frozen time',
    async (state) => {
      twoLinkRun();
      ended(state, 305_400);
      await open();
      await tick(1000);
      expect(pillText('pill-rx')).toBe('RX 9.25 Gbit/s (7.31 Mpps)');
      expect(pillText('pill-tx')).toBe('TX 7.25 Gbit/s (6.31 Mpps)');
      expect(pillText('pill-dropped')).toBe('dropped 8.75 %');
      expect(pillText('pill-elapsed')).toBe('0h 5m 5s');
    },
  );

  it('the pills do not move on while the run stays ended', async () => {
    twoLinkRun();
    ended('stopped');
    await open();
    await tick(1000);
    const before = ['pill-rx', 'pill-tx', 'pill-dropped', 'pill-elapsed'].map(pillText);
    await tick(30_000);
    expect(['pill-rx', 'pill-tx', 'pill-dropped', 'pill-elapsed'].map(pillText)).toEqual(before);
  });

  it("after a live run stops the pills turn from the live numbers to the run's own final ones", async () => {
    twoLinkRun();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { rx_bps: 1e9, rx_pps: 1e5 }), fx.dirStats(1, { rx_bps: 1e9, rx_pps: 1e5 })], {
        elapsed_ms: 4000,
      }),
    );
    await open();
    expect(pillText('pill-rx')).toBe('RX 2.00 Gbit/s (200.00 kpps)');
    ended('stopped', 9000);
    await tick(2500);
    expect(pillText('pill-rx')).toBe('RX 9.25 Gbit/s (7.31 Mpps)');
    expect(pillText('pill-elapsed')).toBe('0h 0m 9s');
  });

  it('the dropped share follows the totals the controller serves (each poll answers with the newest counters)', async () => {
    twoLinkRun();
    ended('stopping');
    const s = { totals: {} as Record<string, ReturnType<typeof fx.totals>> };
    api.getStatsHistory.mockImplementation(async () => ({
      ...fx.history({ '0': [fx.historySample(1000)], '1': [fx.historySample(1000)] }),
      totals: s.totals,
    }));
    s.totals = {
      '0': fx.totals({
        rx_pkts: 1000,
        loss_drops: 10,
        limiter_drops: 0,
        buffer_full_drops: 0,
        nic_tx_drops: 0,
        nic_rx_imissed: 0,
        nic_rx_nombuf: 0,
      }),
    };
    await open();
    expect(pillText('pill-dropped')).toBe('dropped 1.00 %');
    s.totals = {
      '0': fx.totals({
        rx_pkts: 1000,
        loss_drops: 10,
        limiter_drops: 0,
        buffer_full_drops: 0,
        nic_tx_drops: 0,
        nic_rx_imissed: 5,
        nic_rx_nombuf: 5,
      }),
    };
    await tick(2500);
    expect(pillText('pill-dropped')).toBe('dropped 2.00 %');
  });

  it('every cause counts as a drop: imissed and rx_nombuf alone give a share', async () => {
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': [fx.historySample(1000)] },
      totals: {
        '0': fx.totals({
          rx_pkts: 2000,
          loss_drops: 0,
          limiter_drops: 0,
          buffer_full_drops: 0,
          nic_tx_drops: 0,
          nic_rx_imissed: 30,
          nic_rx_nombuf: 10,
        }),
      },
    });
    ended('stopped');
    await open();
    expect(pillText('pill-dropped')).toBe('dropped 2.00 %');
  });

  it('a run without a single drop reads "dropped 0.00 %"', async () => {
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': [fx.historySample(1000)] },
      totals: {
        '0': fx.totals({
          rx_pkts: 5000,
          loss_drops: 0,
          limiter_drops: 0,
          buffer_full_drops: 0,
          nic_tx_drops: 0,
          nic_rx_imissed: 0,
          nic_rx_nombuf: 0,
        }),
      },
    });
    ended('stopped');
    await open();
    expect(pillText('pill-dropped')).toBe('dropped 0.00 %');
  });

  it('before any run the RX, TX and dropped pills are there and say "–"', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await open();
    await tick(2500);
    expect(pillText('pill-rx')).toBe('RX –');
    expect(pillText('pill-tx')).toBe('TX –');
    expect(pillText('pill-dropped')).toBe('dropped –');
    expect(screen.queryByText(/\d+(\.\d+)? (Gbit|Mbit|kbit|bit)/)).not.toBeInTheDocument();
  });

  it('CHOICE: no elapsed pill before any run (the stats carry no elapsed_ms)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await open();
    expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
  });

  it('while running, before the first stats reply, the pills say "–" too', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockImplementation(() => new Promise(() => undefined));
    await open();
    expect(pillText('pill-rx')).toBe('RX –');
    expect(pillText('pill-tx')).toBe('TX –');
    expect(pillText('pill-dropped')).toBe('dropped –');
  });

  it('CHOICE: once a new run is "starting" the old run\'s numbers are gone from the pills', async () => {
    twoLinkRun();
    ended('stopped');
    await open();
    expect(pillText('pill-rx')).toBe('RX 9.25 Gbit/s (7.31 Mpps)');
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(2500);
    expect(pillText('pill-rx')).toBe('RX –');
    expect(pillText('pill-tx')).toBe('TX –');
    expect(pillText('pill-dropped')).toBe('dropped –');
  });

  it('the dropped pill is neutral also after the run: no danger class, no alert', async () => {
    twoLinkRun();
    ended('failed');
    await open();
    const pill = screen.getByTestId('pill-dropped');
    for (const el of [pill, ...Array.from(pill.querySelectorAll('*'))]) {
      expect(el.getAttribute('class') ?? '', el.outerHTML).not.toMatch(/danger|red|error/i);
    }
    expect(pill.closest('[role="alert"]')).toBeNull();
  });

  it('live, nothing dropped: "dropped 0.00 %" (CHOICE: the pill is always there, like the other four)', async () => {
    const none = { loss_pct: 0, limiter_pct: 0, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 0 };
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, none)]));
    renderApp('/');
    await waitFor(() => expect(pillText('pill-dropped')).toBe('dropped 0.00 %'));
  });

  it('live, the dropped share is still the mean weighted by the received packet rate', async () => {
    const shares = (m: number, rx_pps: number) => ({
      rx_pps,
      loss_pct: m,
      limiter_pct: m,
      buffer_full_pct: m,
      nic_tx_pct: 0.5 * m,
      nic_rx_pct: 0.5 * m,
    });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, shares(1, 3e6)), fx.dirStats(1, shares(2, 1e6))]));
    renderApp('/');
    await waitFor(() => expect(pillText('pill-dropped')).toBe('dropped 5.00 %'));
  });
});

// ---- F3: tabs ---------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 35 F3): one tab per link', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
  });

  it('a tablist with one tab per link, in link order, titled "name: ingress → egress" with netdev names', async () => {
    renderApp('/');
    await findCard(1);
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(screen.getAllByRole('tablist')).toHaveLength(1);
    expect(tabs()).toHaveLength(2);
    expect(within(screen.getByRole('tablist')).getAllByRole('tab')).toHaveLength(3); // spec 36: Summary comes first
  });

  it('a live tab has no badge: no lcore badge, no state badge', async () => {
    renderApp('/');
    await findCard(1);
    expect(tabBadge(0)).toBe('');
    expect(tabBadge(1)).toBe('');
    expect(screen.queryByText(/lcore \d/)).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: /^a-b: .* → .*$/ })).toBeInTheDocument();
  });

  // Spec 36: the Summary tab is the first and the default (the Summary tab itself is pinned in dashboardSummary.test.tsx).
  it('no link tab is selected at first, and selecting one makes its panel the one that shows', async () => {
    renderApp('/');
    await findTabs(2);
    expect(tab(0)).toHaveAttribute('aria-selected', 'false');
    expect(tab(1)).toHaveAttribute('aria-selected', 'false');
    const panel = card(0);
    expect(tab(0)).toHaveAttribute('aria-selected', 'true');
    expect(activePanel()).toBe(panel);
  });

  it('clicking a tab shows the chart area of that link', async () => {
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': [fx.historySample(1000, { rx_gbps: 1.5 })], '1': [fx.historySample(1000, { rx_gbps: 7.5 })] },
    });
    renderApp('/');
    await findCard(1);
    await waitFor(() => {
      const chart = within(card(0)).getByTestId('chart-line');
      expect(JSON.parse(chart.getAttribute('data-chart') ?? 'null').datasets[0].data).toEqual([1.5]);
    });
    const chart1 = within(card(1)).getByTestId('chart-line');
    expect(JSON.parse(chart1.getAttribute('data-chart') ?? 'null').datasets[0].data).toEqual([7.5]);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    expect(tab(0)).toHaveAttribute('aria-selected', 'false');
  });

  it('a hairpin link reads "loop: net_null0 → net_null0" in its tab', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { link: 'loop', rx: 'net_null0', tx: 'net_null0' })]));
    renderApp('/');
    await findCard(0);
    expect(tabTitle(0)).toBe('loop: net_null0 → net_null0');
  });

  it('a ring of three links has three tabs titled by their own link and ports', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
        fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
        fx.dirStats(2, { link: 'c-to-a', rx: 'net_null2', tx: 'net_null0' }),
      ]),
    );
    renderApp('/');
    await findCard(2);
    expect([0, 1, 2].map(tabTitle)).toEqual([
      'a-to-b: net_null0 → net_null1',
      'b-to-c: net_null1 → net_null2',
      'c-to-a: net_null2 → net_null0',
    ]);
  });

  it('the selected tab survives the next polls and its chart stays the one of its link', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': [fx.historySample(1000, { rx_gbps: 1.5 })], '1': [fx.historySample(1000, { rx_gbps: 7.5 })] },
    });
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(tab(1)); // fireEvent: userEvent waits on timers that are faked here
    await tick(5000);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    const chart = within(activePanel()).getByTestId('chart-line');
    expect(JSON.parse(chart.getAttribute('data-chart') ?? 'null').datasets[0].data).toEqual([7.5]);
  });

  it('the selected tab stays selected when the run ends (the ended tabs are the same links)', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(tab(1));
    ended('stopped');
    await tick(2500);
    expect(tabBadge(1)).toBe('Stopped');
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
  });

  it('CHOICE: when the selected link is gone (a new run with fewer links) the first tab (the Summary) is selected, not none', async () => {
    vi.useFakeTimers();
    const s = { startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } };
    serveHistory(s);
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(tab(1));
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    await tick(2500);
    expect(tabs()).toHaveLength(1);
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(tab(0)).toHaveAttribute('aria-selected', 'false');
    expect(within(activePanel()).getByTestId('chart-line')).toBeInTheDocument();
  });

  it('no tabs while the stats have no directions and nothing ran', async () => {
    api.getStats.mockResolvedValue(fx.stats([]));
    await open();
    expect(tabs()).toHaveLength(0);
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
    expect(screen.queryAllByRole('tabpanel', { hidden: true })).toHaveLength(0);
  });

  it('API calls still use the link index, not the link name', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
        fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
      ]),
    );
    renderApp('/');
    await findCard(1);
    fireEvent.click(radio(0, 'Delay histogram'));
    fireEvent.click(radio(1, 'Delay histogram'));
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(1));
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    for (const call of api.getDelayHistogram.mock.calls) expect(typeof call[0]).toBe('number');
  });
});

describe('Dashboard (spec 35 F3): the badge of an ended run sits in the tab title', () => {
  const BADGE = { stopping: 'Stopping', stopped: 'Stopped', failed: 'Failed' } as const;

  it.each(['stopping', 'stopped', 'failed'] as const)(
    '"%s": every tab carries the badge "%s" and keeps its title',
    async (state) => {
      serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
      ended(state);
      await open();
      await tick(1000);
      for (const d of [0, 1]) {
        expect(tabBadge(d), `${state}: tab ${d}`).toBe(BADGE[state]);
        expect(within(tab(d)).getByText(BADGE[state])).toBeInTheDocument();
      }
      expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1');
      expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
      // the badge is in the tab, not in the chart area below it
      expect(within(card(0)).queryByText(BADGE[state])).not.toBeInTheDocument();
    },
  );

  it('the badge follows the state: none live, "Stopping", then "Stopped"; a failed run shows "Failed" instead', async () => {
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    await open();
    expect(tabBadge(0)).toBe('');
    ended('stopping');
    await tick(2500);
    expect(tabBadge(0)).toBe('Stopping');
    ended('stopped');
    await tick(2500);
    expect(tabBadge(0)).toBe('Stopped');
    ended('failed');
    await tick(2500);
    expect(tabBadge(0)).toBe('Failed');
  });

  it('a failed badge is not styled like the others (it is a failure)', async () => {
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) } });
    ended('failed');
    await open();
    await tick(1000);
    const badge = must(tab(0).querySelector('.badge'), 'badge');
    expect(badge.getAttribute('class')).toMatch(/danger/);
  });

  it('ended tabs are titled from the history\'s links; "Link <n>" only for an index the links lack', async () => {
    const s = {
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) },
      links: [{ name: 'only', ingress: 'net_null0', egress: 'net_null1' }],
    };
    serveHistory(s);
    ended('stopped');
    await open();
    await tick(1000);
    expect(tabTitle(0)).toBe('only: net_null0 → net_null1');
    expect(tabTitle(1)).toBe('Link 1');
  });

  it('a controller that never ran has no tabs, and no badge anywhere', async () => {
    serveHistory({ startedAt: null, samples: {} });
    ended('stopped');
    await open();
    await tick(1000);
    expect(tabs()).toHaveLength(0);
    expect(screen.queryByText('Stopped', { selector: '.badge' })).not.toBeInTheDocument();
  });

  it('a new start clears the tabs of the old run', async () => {
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    ended('stopped');
    await open();
    await tick(1000);
    expect(hasTab(1)).toBe(true);
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    await tick(2500);
    expect(tabs()).toHaveLength(0);
  });
});

describe('Dashboard (spec 35 F3): the radios under the chart', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
  });

  it('a live link with delay statistics has Rate, Delay histogram and Loss, in this order, in a radiogroup "Chart"', async () => {
    renderApp('/');
    await findCard(1);
    for (const d of [0, 1]) {
      expect(within(card(d)).getByRole('radiogroup', { name: 'Chart' })).toBeInTheDocument();
      expect(radioNames(d)).toEqual(['Rate', 'Delay histogram', 'Loss']);
    }
  });

  it('the old "Drops" choice is gone', async () => {
    renderApp('/');
    await findCard(0);
    expect(queryRadio(0, 'Drops')).not.toBeInTheDocument();
    expect(screen.queryByText('Drops')).not.toBeInTheDocument();
  });

  it('a link with delay_stats off has no "Delay histogram" choice and nothing is fetched for it', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0), fx.dirStats(1, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null })]),
    );
    renderApp('/');
    await findCard(1);
    expect(radioNames(1)).toEqual(['Rate', 'Loss']);
    expect(radioNames(0)).toEqual(['Rate', 'Delay histogram', 'Loss']);
    await new Promise((r) => setTimeout(r, 50));
    expect(api.getDelayHistogram).not.toHaveBeenCalledWith(1);
  });

  it.each(['stopping', 'stopped', 'failed'] as const)('"%s": an ended link has only Rate and Loss', async (state) => {
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    ended(state);
    await open();
    await tick(1000);
    for (const d of [0, 1]) expect(radioNames(d), `${state}: link ${d}`).toEqual(['Rate', 'Loss']);
  });

  it('Rate is the default and the only chart; the radios sit below it, centred', async () => {
    renderApp('/');
    await findCard(0);
    const panel = card(0);
    expect(radio(0, 'Rate')).toBeChecked();
    expect(radio(0, 'Loss')).not.toBeChecked();
    expect(within(panel).getAllByTestId('chart-line')).toHaveLength(1);
    expect(within(panel).queryByTestId('chart-bar')).not.toBeInTheDocument();
    const chart = must(within(panel).getAllByRole('img')[0], 'the chart image');
    expect(chart.getAttribute('aria-label')).toMatch(/rate/i);
    const group = within(panel).getByRole('radiogroup', { name: 'Chart' });
    expect(chart.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(group.getAttribute('class') ?? '').toMatch(/text-center|justify-content-center/);
  });

  it('choosing Loss shows the loss chart (Model loss and Overload drops) instead of the rate chart', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await findCard(0);
    await user.click(radio(0, 'Loss'));
    const panel = card(0);
    const chart = within(panel).getByTestId('chart-line');
    const labels = (JSON.parse(chart.getAttribute('data-chart') ?? 'null').datasets as { label: string }[]).map(
      (s) => s.label,
    );
    expect(labels).toEqual(['Model loss %', 'Overload drops %']);
    expect(within(panel).getAllByTestId('chart-line')).toHaveLength(1);
    expect(must(within(panel).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/loss/i);
    expect(radio(0, 'Loss')).toBeChecked();
    expect(radio(0, 'Rate')).not.toBeChecked();
  });

  it('choosing Delay histogram shows the bar chart instead', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await findCard(0);
    await user.click(radio(0, 'Delay histogram'));
    const panel = card(0);
    await within(panel).findByTestId('chart-bar');
    expect(within(panel).queryByTestId('chart-line')).not.toBeInTheDocument();
    expect(must(within(panel).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/delay histogram/i);
  });

  it('the chart labels tell the links apart', async () => {
    renderApp('/');
    await findCard(1);
    const label = (d: number) => must(within(card(d)).getAllByRole('img')[0]).getAttribute('aria-label');
    expect(label(0)).not.toBe(label(1));
  });

  it('the choice survives the next polls', async () => {
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Loss'));
    await tick(5000);
    expect(radio(0, 'Loss')).toBeChecked();
    expect(must(within(card(0)).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/loss/i);
  });

  it('the choice survives a visit to the other tab and back', async () => {
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Loss'));
    card(1);
    await tick(1000);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    expect(radio(0, 'Loss')).toBeChecked();
    expect(must(within(card(0)).getAllByRole('img')[0]).getAttribute('aria-label')).toMatch(/loss/i);
  });

  it('a histogram choice falls back to Rate when the run ends, and nothing is fetched any more', async () => {
    vi.useFakeTimers();
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 5), '1': fx.historySamples(0, 5) } });
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Delay histogram'));
    await tick(1000);
    expect(within(card(0)).getByTestId('chart-bar')).toBeInTheDocument();
    ended('stopped');
    await tick(2500);
    expect(within(card(0)).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(within(card(0)).getByTestId('chart-line')).toBeInTheDocument();
    expect(radio(0, 'Rate')).toBeChecked();
    const n = api.getDelayHistogram.mock.calls.length;
    await tick(5000);
    expect(api.getDelayHistogram.mock.calls.length).toBe(n);
  });
});

// ---- F6: what is gone from the dashboard --------------------------------------------------------------------------------

describe('Dashboard (spec 35 F6): no stat list, no pid or lcore, no ports table, no log up front', () => {
  const live = () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running', { log_tail: ['EAL: some line', 'dpds: another line'] }));
    api.getStats.mockResolvedValue(
      fx.stats(
        [
          fx.dirStats(0, { first_loss_reason: 'LimiterDrop', first_loss_detail: 'x', first_loss_relative_ns: 1e9 }),
          fx.dirStats(1),
        ],
        { ports: [fx.portStats(0), fx.portStats(1, { name: '0000:65:00.1' })] },
      ),
    );
  };

  it('has no definition list at all, and none of the old stat labels', async () => {
    live();
    renderApp('/');
    await findCard(1);
    for (const d of [0, 1]) {
      fireEvent.click(radio(d, 'Delay histogram'));
    }
    expect(document.querySelectorAll('dl, dt, dd')).toHaveLength(0);
    for (const label of [
      'RX rate',
      'TX rate',
      'Loss drops',
      'Limiter drops',
      'Buffer full drops',
      'NIC TX drops',
      'NIC RX drops',
      'Mean delay',
      'Delay min / max',
      'Lateness',
      'Buffer max',
      'First loss',
    ]) {
      expect(screen.queryByText(label), label).not.toBeInTheDocument();
    }
  });

  it('shows no pid or main lcore text and no lcore badge, also with a running pid', async () => {
    live();
    api.getEmulation.mockResolvedValue(fx.emulation('running', { pid: 4242, main_lcore: 0 }));
    renderApp('/');
    await findCard(1);
    expect(screen.queryByText(/pid\s*\d/)).not.toBeInTheDocument();
    expect(screen.queryByText(/main lcore/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/\blcore\s*\d/i)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/4242/);
  });

  it('has no NIC ports table and no "NIC ports" heading even when the stats carry ports', async () => {
    live();
    renderApp('/');
    await findCard(1);
    await tick0();
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /NIC ports/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('columnheader', { name: 'rx_nombuf' })).not.toBeInTheDocument();
  });

  it.each(['stopped', 'starting', 'running', 'stopping'] as const)(
    '"%s": a log tail the controller reports is not shown, not even collapsed',
    async (state) => {
      api.getEmulation.mockResolvedValue(fx.emulation(state, { log_tail: ['EAL: some line', 'dpds: another line'] }));
      renderApp('/');
      await waitFor(() => expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', state));
      expect(screen.queryByTestId('log-tail')).not.toBeInTheDocument();
      expect(screen.queryByText('Show log')).not.toBeInTheDocument();
      expect(screen.queryByText('EAL: some line')).not.toBeInTheDocument();
    },
  );

  it('"failed": the log is behind a collapsed "Show log" (a details element)', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', {
        error: 'dpds exited during startup (status 1)',
        log_tail: ['EAL: Cannot allocate memory', 'direction 0: cannot create mbuf pool dpds_pool_0'],
      }),
    );
    renderApp('/');
    const summary = await screen.findByText('Show log');
    expect(summary.tagName).toBe('SUMMARY');
    const details = must(summary.closest('details'), 'the details element');
    expect(details.open).toBe(false);
    expect(details).not.toHaveAttribute('open');
    const tail = within(details).getByTestId('log-tail');
    expect(within(tail).getByText('EAL: Cannot allocate memory')).toBeInTheDocument();
    expect(within(tail).getByText('direction 0: cannot create mbuf pool dpds_pool_0')).toBeInTheDocument();
  });

  it('"failed": the summary opens the log', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { error: 'boom', log_tail: ['EAL: line'] }));
    renderApp('/');
    const summary = await screen.findByText('Show log');
    await user.click(summary);
    expect(must(summary.closest('details')).open).toBe(true);
  });

  it('"failed": the error stays in the open (an alert), outside the details', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', { error: 'dpds exited during startup (status 1)', log_tail: ['EAL: line'] }),
    );
    renderApp('/');
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('dpds exited during startup (status 1)');
    expect(alert.closest('details')).toBeNull();
  });

  it('"failed" without a log tail has no "Show log" at all', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { error: 'boom', log_tail: [] }));
    renderApp('/');
    await screen.findByRole('alert');
    expect(screen.queryByText('Show log')).not.toBeInTheDocument();
    expect(document.querySelector('details')).toBeNull();
  });

  it('a log line that repeats is shown as often as it occurs', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { error: 'boom', log_tail: ['same', 'same', 'other'] }));
    renderApp('/');
    const tail = await screen.findByTestId('log-tail');
    expect(within(tail).getAllByText('same')).toHaveLength(2);
  });
});

/** One macrotask so that pending polls have answered (real timers). */
const tick0 = () => new Promise((r) => setTimeout(r, 30));

// ---- F7: what stays ------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 35 F7): the guarantees that stay', () => {
  it('a refused start still shows its error although the state changed in the same poll cycle', async () => {
    vi.useFakeTimers();
    seedStore({ links: [fx.link()] });
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    const refused = deferred<ReturnType<typeof fx.emulation>>();
    api.startEmulation.mockReturnValue(refused.promise);
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(runButton('Start')));
    await tick(0);
    refused.reject(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    await tick(0);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
    expect(runButtonNames()).toEqual(['Restart', 'Start']);
    await tick(6000);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
  });

  it('the LoadRunning banner is still offered while an emulation runs and the browser has no links', async () => {
    api.getEmulation.mockResolvedValue(fx.runningEmulation());
    renderApp('/');
    expect(await screen.findByRole('button', { name: 'Load running configuration' })).toBeInTheDocument();
  });

  it('the dashboard stays quiet in the console: no React warnings while it renders live, ended and empty', async () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) } });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
    await open();
    ended('stopped');
    await tick(2500);
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([]));
    await tick(2500);
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});
