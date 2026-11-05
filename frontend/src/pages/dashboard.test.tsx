import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState, StatsView } from '../api/types';
import {
  card,
  expectState,
  findCard,
  findState,
  hasTab,
  radioNames,
  RUN_BUTTONS,
  runButtonNames,
  runButtons,
  tab,
  tabTitle,
  tabs,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, expectStartBody, renderApp, seedStore, storedDoc, tick } from '../test/helpers';
import { ApiError, api } from '../test/mockApi';
import { must } from '../test/must';

const STATES: EmulationState[] = ['stopped', 'starting', 'running', 'stopping', 'failed'];

/**
 * Spec 28: each link shows ONE chart, chosen with a radio group (Rate is the default). Every test that needs the delay
 * histogram first picks "Delay histogram" in the card(s) with this helper. `fireEvent` so it also works under fake timers.
 */
function showHistogram(...links: number[]): void {
  for (const d of links) fireEvent.click(within(card(d)).getByRole('radio', { name: 'Delay histogram' }));
}

function twoDirections() {
  return fx.stats([fx.dirStats(0), fx.dirStats(1, { rx_pps: 500, tx_pps: 400, rx_bps: 2.5e6, tx_bps: 1.5e3 })]);
}

// v22: Start sends the whole LOCAL configuration (contract: "Start" in the v22 block of src/test/helpers.tsx). The tests of the
// control block start with one stored link; pending restart and "Load running configuration" are in pendingRestart.test.tsx.
describe('Dashboard: emulation state and control', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  // Spec 43 F3. Was: 'shows a state badge for "%s"' (text and data-state of `state-badge`) and, below, 'the state badge is a polite
  // live region'. The badge is gone; the state shows in the run buttons (spec 43 F8: see dashboardP4tg.test.tsx).
  it.each(STATES)('has no state badge in "%s"', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    renderApp('/');
    await findState(state);
    expect(screen.queryByTestId('state-badge')).not.toBeInTheDocument();
    expect(screen.getByTestId('top-pills').querySelector('.badge, [data-state]')).toBeNull();
  });

  // Spec 28: one contextual button instead of a Start and a Stop button that are both always there (details in
  // dashboardP4tg.test.tsx). Spec 43 F8: Start alone, Stop + Restart, or Draining… + Restart (both disabled).
  it.each(STATES)('the run buttons in "%s"', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    renderApp('/');
    await findState(state);
    expect(runButtonNames()).toEqual(RUN_BUTTONS[state]);
    for (const b of runButtons()) {
      if (state === 'stopping') expect(b).toBeDisabled();
      else expect(b).toBeEnabled();
    }
  });

  it('Start calls startEmulation once and shows the new state', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.startEmulation.mockImplementation(async () => {
      api.getEmulation.mockResolvedValue(fx.emulation('starting'));
      return fx.emulation('starting');
    });
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(api.stopEmulation).not.toHaveBeenCalled();
    await findState('starting');
  });

  it('Start sends the whole local configuration: `{links}` without ids, no settings, no presets, nothing else', async () => {
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
    const body = api.startEmulation.mock.calls[0][0] as unknown as Record<string, unknown>;
    expect(Object.keys(body).sort()).toEqual(['links']);
    expect(JSON.stringify(body)).not.toMatch(/"id"|presets|"version"|pending|settings|worker|lcore|eal/);
  });

  it('the start body follows the stored document as it is NOW (an edit made before Start is included)', async () => {
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    seedStore({ links: [fx.loopLink({ id: 7 })] });
    await user.click(btn);
    const body = api.startEmulation.mock.calls[0][0];
    expect(body.links.map((l) => l.name)).toEqual(['loop']);
  });

  it('a link with stages left out is sent without them (null or absent), not with defaults', async () => {
    seedStore({ links: [fx.link({ config: fx.configBare() })] });
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    const c = api.startEmulation.mock.calls[0][0].links[0].config;
    expect(c.delayer ?? null).toBeNull();
    expect(c.limiter ?? null).toBeNull();
    expect(c.loss_model ?? null).toBeNull();
  });

  it('without local links Start sends nothing and says so in an alert', async () => {
    localStorage.clear();
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent(/no links|nothing to start/i);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('with an empty links list stored the same applies', async () => {
    seedStore({ links: [] });
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent(/no links|nothing to start/i);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('starting does not write the document', async () => {
    const before = localStorage.getItem('dpds.config.v1');
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
  });

  it('Stop calls stopEmulation once', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^stop$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('a click while the start request is in flight does not send a second request', async () => {
    const user = userEvent.setup();
    const d = deferred<ReturnType<typeof fx.emulation>>();
    api.startEmulation.mockReturnValue(d.promise);
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    await user.click(btn);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(btn).toBeDisabled();
    d.resolve(fx.emulation('starting'));
  });

  it.each([
    [400, 'links[0].config.delayer.std_ns must be finite and >= 0'],
    [400, 'nothing to start'],
    [409, 'an emulation is already running'],
  ])('shows the message of a refused start (%i)', async (status, message) => {
    const user = userEvent.setup();
    api.startEmulation.mockRejectedValue(new ApiError(status, message));
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent(message);
    expect(btn).toBeEnabled(); // can be retried
  });

  it('shows the API error message when start is refused (409)', async () => {
    const user = userEvent.setup();
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a host uplink');
  });

  it('shows the API error message when stop fails', async () => {
    const user = userEvent.setup();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.stopEmulation.mockRejectedValue(new ApiError(500, 'cannot signal child'));
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^stop$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent('cannot signal child');
  });

  // Spec 35 F6. Was: the log tail was always on the page; it is now only shown for `failed`, inside a collapsed "Show log".
  it('shows the error when failed and keeps the log tail behind a collapsed "Show log"', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', {
        error: 'dpds exited during startup (status 1)',
        log_tail: ['EAL: Cannot allocate memory', 'direction 0: cannot create mbuf pool dpds_pool_0'],
      }),
    );
    renderApp('/');
    expect(await screen.findByText(/dpds exited during startup \(status 1\)/)).toBeInTheDocument();
    expect(must(screen.getByText('Show log').closest('details'), 'details').open).toBe(false);
    const tail = screen.getByTestId('log-tail');
    expect(within(tail).getByText('EAL: Cannot allocate memory')).toBeInTheDocument();
    expect(within(tail).getByText('direction 0: cannot create mbuf pool dpds_pool_0')).toBeInTheDocument();
  });

  it('shows no error alert and no log tail while stopped without error', async () => {
    renderApp('/');
    await screen.findByTestId('run-buttons');
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByTestId('log-tail')).not.toBeInTheDocument();
  });

  it('shows an alert when the controller cannot be reached', async () => {
    api.getEmulation.mockRejectedValue(new Error('Failed to fetch'));
    renderApp('/');
    expect(await screen.findByRole('alert')).toHaveTextContent(/failed to fetch/i);
  });
});

// Spec 35 F3/F6. Was: 'Dashboard: per-direction cards', one card per link with a stat list (rates, drop shares per cause, mean
// delay +- std, lateness, buffer max). The cards are tabs now and the stat list is gone (see dashboardSpec35.test.tsx); the
// tests of the removed list (RX/TX rate, scaled units, drop shares, delay values, n/a, zero values) are removed with it.
describe('Dashboard: one tab per link', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('renders one tab per link with link and interface names (netdevs, v16)', async () => {
    renderApp('/');
    await findCard(1);
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(hasTab(2)).toBe(false);
  });

  it('shows no tabs while the stats have no directions', async () => {
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(tabs()).toHaveLength(0);
  });

  // Spec 28: a radio selector shows one chart per link; Rate is the default. Was: findAllByTestId('chart-line') with the
  // rate and the drops chart both on the page.
  it('draws the rate line chart once directions exist (one chart per link, Rate by default)', async () => {
    renderApp('/');
    await findCard(0);
    const charts = await within(card(0)).findAllByTestId('chart-line');
    expect(charts).toHaveLength(1);
    const data = JSON.parse(charts[0].getAttribute('data-chart') ?? 'null');
    expect(data.datasets.length).toBeGreaterThanOrEqual(1);
  });
});

describe('Dashboard: staleness', () => {
  it('shows a stale indicator when stats.stale', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: 6500 }));
    renderApp('/');
    expect(await screen.findByTestId('stale-indicator')).toHaveTextContent(/stale/i);
  });

  it('shows no indicator for fresh stats', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { stale: false, age_ms: 100 }));
    renderApp('/');
    await findCard(0);
    expect(screen.queryByTestId('stale-indicator')).not.toBeInTheDocument();
  });

  it('the indicator appears and disappears with later polls', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    await tick(0);
    expect(screen.queryByTestId('stale-indicator')).not.toBeInTheDocument();
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: 7000 }));
    await tick(2500); // one poll interval is <= 2 s
    expect(screen.getByTestId('stale-indicator')).toBeInTheDocument();
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    await tick(2500);
    expect(screen.queryByTestId('stale-indicator')).not.toBeInTheDocument();
  });
});

describe('Dashboard: stale badge only while running (F1)', () => {
  /** Renders the dashboard in `state` with these stats; true iff the stale indicator is shown. */
  async function staleShown(state: EmulationState, view: StatsView): Promise<boolean> {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    api.getStats.mockResolvedValue(view);
    renderApp('/');
    await tick(0);
    await tick(0);
    expectState(state);
    expect(api.getStats).toHaveBeenCalled();
    return screen.queryByTestId('stale-indicator') !== null;
  }

  // The payload the controller really sends for a stopped emulation (no age_ms at all).
  const STOPPED_PAYLOAD: StatsView = { directions: [], stale: true };

  it('the real stopped payload {"directions":[],"ports":[],"stale":true} shows no badge', async () => {
    expect(await staleShown('stopped', STOPPED_PAYLOAD)).toBe(false);
  });

  it.each(['stopped', 'starting', 'stopping', 'failed'] as const)(
    'no badge in "%s" even with stale: true and age_ms present',
    async (state) => {
      expect(await staleShown(state, fx.stats([], { stale: true, age_ms: 6500 }))).toBe(false);
    },
  );

  it('no badge while starting with the stopped payload either', async () => {
    expect(await staleShown('starting', STOPPED_PAYLOAD)).toBe(false);
  });

  it('shows the badge while running with stale: true and age_ms (control for the cases above)', async () => {
    expect(await staleShown('running', fx.stats([], { stale: true, age_ms: 6500 }))).toBe(true);
  });

  it('no badge while running when age_ms is absent', async () => {
    expect(await staleShown('running', STOPPED_PAYLOAD)).toBe(false);
  });

  it('no badge while running when age_ms is null (never polled)', async () => {
    expect(await staleShown('running', fx.stats([], { stale: true, age_ms: null }))).toBe(false);
  });

  it('the badge disappears again when the emulation stops although the stale payload stays', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: 6500 }));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('stale-indicator')).toBeInTheDocument();
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(STOPPED_PAYLOAD);
    await tick(2500);
    expectState('stopped');
    expect(screen.queryByTestId('stale-indicator')).not.toBeInTheDocument();
  });
});

describe('Dashboard: delay histogram', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('renders a bar chart from the bins of each direction', async () => {
    renderApp('/');
    await findCard(0);
    await findCard(1);
    showHistogram(0, 1);
    const chart = await within(card(0)).findByTestId('chart-bar');
    const data = JSON.parse(chart.getAttribute('data-chart') ?? 'null');
    expect(data.datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    expect(data.labels).toHaveLength(5);
    const chart1 = await within(card(1)).findByTestId('chart-bar');
    expect(JSON.parse(chart1.getAttribute('data-chart') ?? 'null').datasets[0].data).toEqual([1, 10, 100, 40, 3]);
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    expect(api.getDelayHistogram).toHaveBeenCalledWith(1);
  });

  it('works for a large bin count (500)', async () => {
    const bins = Array.from({ length: 500 }, (_, i) => i);
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { bins, edges_ns: Array.from({ length: 501 }, (_, i) => i * 10) }),
    );
    renderApp('/');
    await findCard(0);
    showHistogram(0);
    const chart = await within(card(0)).findByTestId('chart-bar');
    const data = JSON.parse(chart.getAttribute('data-chart') ?? 'null');
    expect(data.datasets[0].data).toHaveLength(500);
    expect(data.labels).toHaveLength(500);
  });

  // Spec 35 F4. Was: 'shows the underflow and overflow counts' (under/overflow text under the histogram; removed).
  it('shows no underflow or overflow text under the histogram', async () => {
    renderApp('/');
    await findCard(0);
    showHistogram(0);
    await within(card(0)).findByTestId('chart-bar');
    expect(card(0)).not.toHaveTextContent(/Underflow|Overflow/);
  });

  it('hides the chart and does not fetch it for a direction with delay_stats: false', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0),
        fx.dirStats(1, {
          delay_stats: false,
          delay_mean_ns: null,
          delay_std_ns: null,
          delay_min_ns: null,
          delay_max_ns: null,
          lateness_mean_ns: null,
          lateness_max_ns: null,
        }),
      ]),
    );
    renderApp('/');
    await findCard(0);
    await findCard(1);
    showHistogram(0);
    expect(within(card(1)).queryByRole('radio', { name: 'Delay histogram' })).not.toBeInTheDocument(); // spec 28: no choice
    await within(card(0)).findByTestId('chart-bar');
    expect(within(card(1)).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(api.getDelayHistogram).not.toHaveBeenCalledWith(1);
  });

  it('survives a rejected histogram request (card stays, chart missing)', async () => {
    api.getDelayHistogram.mockRejectedValue(new ApiError(404, 'no histogram'));
    renderApp('/');
    await findCard(0);
    showHistogram(0);
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalled());
    expect(within(card(0)).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(radioNames(0)).toContain('Traffic rates'); // the chart area is still there
  });
});

describe('Dashboard: polling (fake timers)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('polls emulation and stats repeatedly (interval <= 2 s)', async () => {
    renderApp('/');
    await tick(0);
    await tick(0);
    showHistogram(0); // the histogram polls while its chart is the selected one
    await tick(0);
    const e0 = api.getEmulation.mock.calls.length;
    const s0 = api.getStats.mock.calls.length;
    expect(e0).toBeGreaterThanOrEqual(1);
    expect(s0).toBeGreaterThanOrEqual(1);
    await tick(5000);
    expect(api.getEmulation.mock.calls.length).toBeGreaterThanOrEqual(e0 + 2);
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(s0 + 2);
    expect(api.getDelayHistogram.mock.calls.filter(([d]) => d === 0).length).toBeGreaterThanOrEqual(3);
  });

  function slow<A extends unknown[], R>(result: R, ms: number, probe: { active: number; max: number; calls: number }) {
    return async (..._args: A): Promise<R> => {
      probe.calls += 1;
      probe.active += 1;
      probe.max = Math.max(probe.max, probe.active);
      await new Promise((r) => setTimeout(r, ms));
      probe.active -= 1;
      return result;
    };
  }

  it.each(['getEmulation', 'getStats', 'getDelayHistogram'] as const)(
    'never has two %s requests in flight (request takes 3.5 s)',
    async (name) => {
      const probe = { active: 0, max: 0, calls: 0 };
      if (name === 'getEmulation') api.getEmulation.mockImplementation(slow(fx.emulation('running'), 3500, probe));
      if (name === 'getStats') api.getStats.mockImplementation(slow(twoDirections(), 3500, probe));
      if (name === 'getDelayHistogram') {
        api.getDelayHistogram.mockImplementation(async (dir) => {
          if (dir !== 0) return fx.histogram(dir);
          return slow(fx.histogram(0), 3500, probe)();
        });
      }
      renderApp('/');
      await tick(0); // let the first stats arrive and the direction cards mount (act batches updates)
      await tick(0);
      // (with a slow getStats the cards never appear in time, so only the histogram variant picks the chart)
      if (name === 'getDelayHistogram') {
        showHistogram(0); // only the shown tab polls its histogram, and the probe watches link 0
      }
      for (let i = 0; i < 30; i += 1) await tick(1000);
      expect(probe.max).toBe(1);
      expect(probe.calls).toBeGreaterThanOrEqual(3); // polling keeps going
      expect(probe.calls).toBeLessThanOrEqual(9); // and does not fire while one is pending (30 s / 3.5 s)
    },
  );

  it('a request that never settles blocks only its own poller, not the others', async () => {
    const never = deferred<ReturnType<typeof fx.emulation>>();
    api.getEmulation.mockReturnValue(never.promise);
    renderApp('/');
    await tick(10000);
    expect(api.getEmulation).toHaveBeenCalledTimes(1);
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(3);
  });

  it('keeps polling after a failed request and recovers', async () => {
    api.getStats.mockReset();
    api.getStats.mockRejectedValueOnce(new Error('boom')).mockResolvedValue(twoDirections());
    const spy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    renderApp('/');
    await tick(0);
    expect(hasTab(0)).toBe(false);
    await tick(4000);
    expect(hasTab(0)).toBe(true);
    spy.mockRestore();
  });

  it('stops polling after unmount', async () => {
    const view = renderApp('/');
    await tick(3000);
    view.unmount();
    // spec 29: the history poll stops too
    const polled = [api.getEmulation, api.getStats, api.getDelayHistogram, api.getStatsHistory];
    const counts = polled.map((m) => m.mock.calls.length);
    expect(api.getStatsHistory.mock.calls.length).toBeGreaterThanOrEqual(1);
    await tick(20000);
    expect(polled.map((m) => m.mock.calls.length)).toEqual(counts);
  });

  it('a response that arrives after unmount does not break anything', async () => {
    const d = deferred<ReturnType<typeof twoDirections>>();
    api.getStats.mockReturnValue(d.promise);
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const view = renderApp('/');
    await tick(0);
    view.unmount();
    d.resolve(twoDirections());
    await tick(5000);
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});

// Spec 35 F6. Removed: 'Dashboard: delay min/max and first loss (F2)' (delay min / max and first loss belonged to the stat list
// of the card, which is gone) and 'Dashboard: NIC ports table (F2)' (the table moved to the Interfaces page, see
// interfacesPorts.test.tsx).

describe('Dashboard: visible errors (F3)', () => {
  const quiet = () => vi.spyOn(console, 'error').mockImplementation(() => undefined);

  it('a failing stats poll shows a warning with the message (not only in the console)', async () => {
    const spy = quiet();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockRejectedValue(new ApiError(502, 'controller unreachable'));
    renderApp('/');
    const warning = await screen.findByTestId('stats-warning');
    expect(warning).toHaveTextContent('controller unreachable');
    spy.mockRestore();
  });

  it('the warning can be dismissed', async () => {
    const spy = quiet();
    const user = userEvent.setup();
    api.getStats.mockRejectedValue(new ApiError(502, 'controller unreachable'));
    renderApp('/');
    const warning = await screen.findByTestId('stats-warning');
    await user.click(within(warning).getByRole('button', { name: /close|dismiss/i }));
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
    spy.mockRestore();
  });

  it('the warning goes away by itself once a later poll succeeds', async () => {
    const spy = quiet();
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockReset();
    api.getStats.mockRejectedValueOnce(new Error('boom')).mockResolvedValue(twoDirections());
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('stats-warning')).toHaveTextContent('boom');
    await tick(2500);
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
    expect(hasTab(0)).toBe(true);
    spy.mockRestore();
  });

  it('no warning while the stats polls succeed', async () => {
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    await findCard(0);
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
  });

  it('a failed histogram fetch shows a small note in that card, with the reason', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    api.getDelayHistogram.mockImplementation(async (dir) => {
      if (dir === 0) throw new ApiError(500, 'histogram read failed');
      return fx.histogram(dir);
    });
    renderApp('/');
    await findCard(0);
    await findCard(1);
    showHistogram(0, 1);
    const note = await within(card(0)).findByTestId('histogram-error');
    expect(note).toHaveTextContent(/histogram/i);
    expect(note).toHaveTextContent('histogram read failed');
    expect(within(card(0)).queryByTestId('chart-bar')).not.toBeInTheDocument();
    // the other direction is unaffected
    await within(card(1)).findByTestId('chart-bar');
    expect(within(card(1)).queryByTestId('histogram-error')).not.toBeInTheDocument();
  });

  it('the histogram note disappears and the chart returns when a later fetch succeeds', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    // the first fetch made after the histogram was chosen fails (spec 28: it is one choice of the card's selector)
    let chosen = false;
    let failed = false;
    api.getDelayHistogram.mockImplementation(async (dir) => {
      if (chosen && !failed) {
        failed = true;
        throw new ApiError(500, 'transient');
      }
      return fx.histogram(dir);
    });
    renderApp('/');
    await tick(0);
    await tick(0);
    card(0);
    chosen = true;
    showHistogram(0);
    await tick(0);
    expect(within(card(0)).getByTestId('histogram-error')).toBeInTheDocument();
    await tick(2500);
    expect(within(card(0)).queryByTestId('histogram-error')).not.toBeInTheDocument();
    expect(within(card(0)).getByTestId('chart-bar')).toBeInTheDocument();
  });

  it('no histogram note for a direction with delay_stats: false (nothing was fetched)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null })]),
    );
    renderApp('/');
    await findCard(0);
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(within(card(0)).queryByTestId('histogram-error')).not.toBeInTheDocument();
  });
});

describe('Dashboard: accessibility and labels (F4)', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  // Spec 28: one chart per card at a time. Was: the rate, drops and histogram charts all on the page (three images).
  // Spec 35: "Drops" is "Loss"; the donut below the chart is not part of the check, so the chart is the first image of the
  // chart area (the old test required exactly one image).
  it('every chart sits in a role="img" element with an aria-label (rate, loss, histogram: one at a time)', async () => {
    renderApp('/');
    await findCard(0);
    for (const [choice, re, id] of [
      ['Traffic rates', /rate/i, 'chart-line'],
      ['Packet loss', /loss/i, 'chart-line'],
      ['Delay histogram', /histogram/i, 'chart-bar'],
    ] as const) {
      fireEvent.click(within(card(0)).getByRole('radio', { name: choice }));
      const chart = await within(card(0)).findByTestId(id);
      const img = must(within(card(0)).getAllByRole('img')[0], 'the chart image');
      expect(within(card(0)).queryAllByTestId(/^chart-(line|bar)$/), choice).toHaveLength(1);
      expect(img.getAttribute('aria-label')?.trim()).toBeTruthy();
      expect(img.getAttribute('aria-label')).toMatch(re);
      // the chart canvas is the content of that image
      expect(img.contains(chart)).toBe(true);
    }
  });

  it('the chart labels tell the directions apart', async () => {
    renderApp('/');
    await findCard(0);
    await findCard(1);
    const label = (d: number, re: RegExp) =>
      within(card(d))
        .getAllByRole('img')
        .map((i) => i.getAttribute('aria-label') ?? '')
        .find((n) => re.test(n)) ?? '';
    expect(label(0, /rate/i)).not.toBe(label(1, /rate/i));
  });

  it('the emulation error is announced as role="alert"', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', {
        error: 'dpds exited during startup (status 1)',
        log_tail: ['EAL: Cannot allocate memory'],
      }),
    );
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    expect(await screen.findByRole('alert')).toHaveTextContent('dpds exited during startup (status 1)');
  });

  // Spec 35 F3: the title is the tab's (it was the card header's).
  it('tabs are titled "<link>: <ingress label> -> <egress label>" (v16: the netdev only)', async () => {
    renderApp('/');
    await findCard(1);
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(within(tab(0)).getByText('a-b: enp101s0f0 → enp101s0f1')).toBeInTheDocument();
  });

  it('the old "Direction d: rx -> tx" title is gone', async () => {
    renderApp('/');
    await findCard(0);
    expect(card(0)).not.toHaveTextContent(/Direction \d/);
    expect(tab(0)).not.toHaveTextContent(/Direction \d/);
    expect(screen.queryByText(/Direction \d:/)).not.toBeInTheDocument();
  });

  it('a hairpin link reads "loop: net_null0 -> net_null0"', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { link: 'loop', rx: 'net_null0', tx: 'net_null0' })]));
    renderApp('/');
    await findCard(0);
    expect(within(tab(0)).getByText('loop: net_null0 → net_null0')).toBeInTheDocument();
  });

  it('the title uses the link name of the stats entry, ingress as rx and egress as tx (ring of three links)', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
        fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
        fx.dirStats(2, { link: 'c-to-a', rx: 'net_null2', tx: 'net_null0' }),
      ]),
    );
    renderApp('/');
    expect(await screen.findByText('a-to-b: net_null0 → net_null1')).toBeInTheDocument();
    expect(screen.getByText('b-to-c: net_null1 → net_null2')).toBeInTheDocument();
    expect(screen.getByText('c-to-a: net_null2 → net_null0')).toBeInTheDocument();
    expect(tab(2)).toHaveTextContent('c-to-a: net_null2 → net_null0');
  });

  it('API calls still use the direction index, not the link name', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
        fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
      ]),
    );
    renderApp('/');
    await screen.findByText('b-to-c: net_null1 → net_null2');
    showHistogram(0, 1);
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(1));
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    for (const call of api.getDelayHistogram.mock.calls) expect(typeof call[0]).toBe('number');
  });
});

describe('Dashboard: port labels are the netdev, device name as fallback (v16, replaces v12 "PCI (netdev)")', () => {
  const AB = 'a-b: enp101s0f0 → enp101s0f1';
  const PLAIN_AB = 'a-b: 0000:65:00.0 → 0000:65:00.1';

  it('the tab title shows the netdev of ingress and egress and no PCI address (one element, whole label)', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    await findCard(0);
    const title = await within(tab(0)).findByText(AB);
    expect(title.textContent?.trim()).toBe(AB);
    expect(tab(0).textContent).not.toMatch(/0000:65:00\.[01] \(/); // never "PCI (netdev)" any more
    expect(card(0).textContent).not.toMatch(/0000:65:00\.[01] \(/);
  });

  it('a hairpin on a real NIC shows the label twice', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { link: 'hair', rx: '0000:65:00.1', tx: '0000:65:00.1' })]),
    );
    renderApp('/');
    expect(await screen.findByText('hair: enp101s0f1 → enp101s0f1')).toBeInTheDocument();
  });

  it('a vdev port shows its device name, next to a NIC shown by netdev', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { link: 'mix', rx: 'net_null0', tx: '0000:65:00.0' })]));
    renderApp('/');
    expect(await screen.findByText('mix: net_null0 → enp101s0f0')).toBeInTheDocument();
  });

  it('a NIC without netdev (vfio-bound) falls back to its PCI address', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({ pci: '0000:65:00.0', netdev: null }),
      fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' }),
    ]);
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    expect(await screen.findByText('a-b: 0000:65:00.0 → enp101s0f1')).toBeInTheDocument();
  });

  it('a port missing from the interface list shows only its name', async () => {
    api.getInterfaces.mockResolvedValue([fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' })]);
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    expect(await screen.findByText('a-b: 0000:65:00.0 → enp101s0f1')).toBeInTheDocument();
  });

  it('without rx/tx in the stats entry the plan of the emulation supplies the ports (labelled the same way)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { link: null, rx: null, tx: null })]));
    renderApp('/');
    expect(await screen.findByText(AB)).toBeInTheDocument();
  });

  it('a failing getInterfaces falls back to device names: no alert, no warning, cards unaffected', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    expect(await screen.findByText(PLAIN_AB)).toBeInTheDocument();
    expect(screen.getByText('b-a: 0000:65:00.1 → 0000:65:00.0')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/sysfs unreadable/);
    expect(radioNames(0)).toContain('Traffic rates'); // the tabs and their chart areas are unaffected
  });

  it('a slow interface list does not hold the cards back: plain names first, then the labels', async () => {
    const d = deferred<ReturnType<typeof fx.interfaces>>();
    api.getInterfaces.mockReturnValue(d.promise);
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    expect(await screen.findByText(PLAIN_AB)).toBeInTheDocument();
    d.resolve(fx.interfaces());
    expect(await screen.findByText(AB)).toBeInTheDocument();
    expect(screen.queryByText(PLAIN_AB)).not.toBeInTheDocument();
  });

  it('the interface list is fetched once per page load, not on every poll', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
    const polls = api.getStats.mock.calls.length;
    await tick(10000);
    expect(api.getStats.mock.calls.length).toBeGreaterThanOrEqual(polls + 4); // polling went on...
    expect(api.getInterfaces).toHaveBeenCalledTimes(1); // ...without asking for interfaces again
    expect(screen.getByText(AB)).toBeInTheDocument();
  });

  it('a failed interface fetch is not retried on every poll either', async () => {
    vi.useFakeTimers();
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    await tick(0);
    await tick(10000);
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
    expect(screen.getByText(PLAIN_AB)).toBeInTheDocument();
  });

  // Spec 35 F6. Removed: the 'NIC ports table' sub-block (port names in the ports table); it moved to interfacesPorts.test.tsx.
});
