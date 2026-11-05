import { fireEvent, screen, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { EmulationState, EmulationView } from '../api/types';
import { allTabs, panelOf, summaryTab, tab, tabBadge, tabs, tabTitle } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, renderApp, seedStore, tick } from '../test/helpers';
import { ApiError, api, fakeController, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 54 (user 2026-11-05: "the dashboard should not show 'stopped' next to the links which I can select. Further when starting,
// it should not get white and then render. It should still show it and also display some loading like P4TG does. Maybe we can
// display a 'DPDS' logo as text below the loading"). Spec: .git/claude-specs/spec54-no-tab-badge-start-overlay.md.
//   L1 no Stopping/Stopped/Failed badge in the tab titles.
//   L3 from the Start/Restart click until `running` the dashboard content stays in the DOM (never removed, not remounted) under
//      the Loader's overlay (components/Loader.tsx; class names: header of components/loader.test.tsx); a refused or failed start
//      removes the overlay and shows the error as before.
//   L4 before the first /api/emulation answer the non-overlay loader shows instead of an empty page.
// CHOICES (spec silent): the overlay of a Restart starts at the Restart click (through stopping and stopped) and ends at `running`,
// a failed start or a refused one; a page opened while the emulation is `starting` shows the overlay; a plain Stop shows none;
// "the chart area stays" is tested as: the tabpanel elements of the views still are connected to the document (same elements).

const LINKS = [fx.link(), fx.linkRev()];
const TITLES = ['a-b: enp101s0f0 → enp101s0f1', 'b-a: enp101s0f1 → enp101s0f0'];
const STOPPED_STATS = () => fx.stats([], { stale: true, age_ms: undefined });
const LIVE_STATS = () => fx.stats([fx.dirStats(0), fx.dirStats(1)]);

const overlay = () => document.querySelector('.loader-overlay');
const block = () => document.querySelector('.loader-block');
const startBtn = () => screen.getByRole('button', { name: /^start$/i });
const restartBtn = () => screen.getByRole('button', { name: /^restart$/i });

const rxOf = (panel: HTMLElement): number[] => {
  const data = JSON.parse(within(panel).getByTestId('chart-line').getAttribute('data-chart') ?? 'null');
  return must(
    data.datasets.find((s: { label: string }) => s.label === 'RX'),
    'RX dataset',
  ).data;
};

/** The elements that must survive the start: the tab list and every tab with its chart area (Summary, link 0, link 1). */
function snapshot() {
  const all = allTabs();
  return { all, panels: all.map(panelOf), list: screen.getByRole('tablist') };
}
type Snapshot = ReturnType<typeof snapshot>;

/** Same tabs, same tabpanel elements, still in the document, the overlay over them. */
function expectKept(before: Snapshot, covered: boolean, what: string) {
  expect(screen.getByRole('tablist'), what).toBe(before.list);
  expect(allTabs().length, `${what}: tab count`).toBe(before.all.length);
  for (const [i, p] of before.panels.entries()) expect(p.isConnected, `${what}: panel ${i} in the document`).toBe(true);
  for (const [i, t] of before.all.entries()) expect(t.isConnected, `${what}: tab ${i} in the document`).toBe(true);
  expect(
    tabs().map((_, d) => tabTitle(d)),
    `${what}: titles`,
  ).toEqual(TITLES);
  if (covered) {
    const layer = must(overlay(), `${what}: the overlay`);
    const root = must(layer.closest('.loader-content'), `${what}: the loader root`);
    expect(root, what).toContainElement(before.list);
    expect(root, what).toContainElement(before.panels[0]);
    expect(block(), `${what}: no non-overlay loader`).toBeNull();
  } else {
    expect(overlay(), `${what}: no overlay`).toBeNull();
  }
}

async function open(initial: EmulationState = 'stopped') {
  vi.useFakeTimers();
  const c = fakeController(initial);
  api.getStats.mockResolvedValue(initial === 'running' ? LIVE_STATS() : STOPPED_STATS());
  renderApp('/');
  await tick(0);
  await tick(0);
  return c;
}

/** Polls for `seconds` half-seconds, checking the page after each. */
async function through(before: Snapshot, covered: boolean, what: string, steps = 4) {
  for (let i = 0; i < steps; i++) {
    await tick(500);
    expectKept(before, covered, `${what} (+${(i + 1) * 500} ms)`);
  }
}

const runsWithHistory = () =>
  serveHistory({
    startedAt: 1762000000,
    samples: { '0': fx.historySamples(0, 5, { rx_gbps: 1.5 }), '1': fx.historySamples(0, 5, { rx_gbps: 2.5 }) },
  });

beforeEach(() => {
  seedStore({ links: LINKS });
});

// ---- L1 -----------------------------------------------------------------------------------------------------------------------

describe('spec 54 L1: no state badge in the tab titles', () => {
  it('through running, stopping, stopped and failed every tab is exactly its label, with no badge', async () => {
    runsWithHistory();
    await open('running');
    for (const state of ['running', 'stopping', 'stopped', 'failed'] as const) {
      api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: state === 'running' ? fx.plan() : [] }));
      api.getStats.mockResolvedValue(state === 'running' ? LIVE_STATS() : STOPPED_STATS());
      await tick(2500);
      expect(summaryTab().textContent, state).toBe('Summary');
      expect(
        tabs().map((t) => t.textContent),
        state,
      ).toEqual(TITLES);
      for (const t of allTabs()) expect(t.querySelector('.badge'), state).toBeNull();
      expect(tabBadge(0), state).toBe('');
      expect(within(screen.getByRole('tablist')).queryByText(/Stopping|Stopped|Failed/), state).not.toBeInTheDocument();
    }
  });
});

// ---- L3: Start -----------------------------------------------------------------------------------------------------------------

describe('spec 54 L3: Start keeps the dashboard rendered under the overlay', () => {
  it('shows no overlay and no non-overlay loader on a settled stopped dashboard', async () => {
    await open();
    expect(overlay()).toBeNull();
    expect(block()).toBeNull();
    expect(allTabs()).toHaveLength(3);
  });

  it('from the click (request pending) through `starting` until `running` the tabs and chart areas stay, covered; then the overlay is gone', async () => {
    const c = await open();
    const before = snapshot();
    expect(rxOf(before.panels[1])).toEqual([]); // the before-run placeholder chart
    const reply = deferred<EmulationView>();
    api.startEmulation.mockImplementation(() => reply.promise);
    fireEvent.click(startBtn());
    await tick(0);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expectKept(before, true, 'request pending');
    c.state = 'starting';
    reply.resolve(fx.emulation('starting'));
    await tick(0);
    expectKept(before, true, 'answered with starting');
    await through(before, true, 'starting', 6);
    expect(rxOf(before.panels[1])).toEqual([]);
    // the run is up
    c.state = 'running';
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
    expect(block()).toBeNull();
    expect(allTabs()).toHaveLength(3);
    expect(tabs().map((t) => t.textContent)).toEqual(TITLES);
  });

  it('with a link tab selected, that tab stays selected and its chart area stays the whole time', async () => {
    const c = await open();
    fireEvent.click(tab(1));
    const before = snapshot();
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    fireEvent.click(startBtn());
    await tick(0);
    expectKept(before, true, 'request pending');
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    expect(c.state).toBe('starting');
    await through(before, true, 'starting', 4);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    c.state = 'running';
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
  });

  it('after an ended run the old tabs and their final charts stay while the next one starts', async () => {
    runsWithHistory();
    const c = await open('stopped');
    await tick(1500);
    const before = snapshot();
    expect(rxOf(before.panels[1])).toEqual(Array(5).fill(1.5));
    fireEvent.click(startBtn());
    await tick(0);
    expect(c.state).toBe('starting');
    expectKept(before, true, 'request answered');
    for (let i = 0; i < 4; i++) {
      await tick(500);
      expectKept(before, true, `starting +${(i + 1) * 500} ms`);
      expect(rxOf(before.panels[1]), 'previous content of link 0').toEqual(Array(5).fill(1.5));
      expect(rxOf(before.panels[2]), 'previous content of link 1').toEqual(Array(5).fill(2.5));
    }
    c.state = 'running';
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
  });

  it('a start the controller refuses (409) removes the overlay and shows the error, the dashboard stays', async () => {
    await open();
    const before = snapshot();
    const reply = deferred<EmulationView>();
    api.startEmulation.mockImplementation(() => reply.promise);
    fireEvent.click(startBtn());
    await tick(0);
    expectKept(before, true, 'request pending');
    reply.reject(new ApiError(409, 'cannot start while the emulation is stopping'));
    await tick(0);
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('cannot start while the emulation is stopping');
    expectKept(before, false, 'after the refusal');
    await through(before, false, 'later', 3);
    expect(startBtn()).toBeEnabled();
  });

  it('a start that is refused at once (no links configured) shows the error and no overlay', async () => {
    seedStore({ links: [] });
    await open();
    fireEvent.click(startBtn());
    await tick(0);
    await tick(1000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent(/No links configured/);
  });

  it('a start that is accepted but ends `failed` removes the overlay and shows the emulation error', async () => {
    const c = await open();
    const before = snapshot();
    fireEvent.click(startBtn());
    await tick(0);
    expectKept(before, true, 'starting');
    c.state = 'failed';
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { directions: [], error: 'dpds exited (status 1)' }));
    await tick(2500);
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('dpds exited (status 1)');
    expect(allTabs()).toHaveLength(3);
    expect(before.panels[0].isConnected).toBe(true);
  });

  it.each(['stopped', 'running', 'stopping', 'failed'] as const)(
    'no overlay when the page opens in "%s"',
    async (state) => {
      await open(state);
      await tick(2500);
      expect(overlay()).toBeNull();
      expect(block()).toBeNull();
    },
  );

  it('a page opened while the emulation is `starting` shows the overlay until it runs', async () => {
    const c = await open('starting');
    expect(overlay()).not.toBeNull();
    await tick(2500);
    expect(overlay()).not.toBeNull();
    c.state = 'running';
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
  });

  it('a plain Stop shows no overlay', async () => {
    runsWithHistory();
    await open('running');
    fireEvent.click(screen.getByRole('button', { name: /^stop$/i }));
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(overlay()).toBeNull();
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
  });
});

// ---- L3: Restart -----------------------------------------------------------------------------------------------------------------

describe('spec 54 L3: Restart keeps the dashboard rendered under the overlay', () => {
  async function openRunning() {
    runsWithHistory();
    const c = await open('running');
    await tick(1500);
    fireEvent.click(tab(1));
    return c;
  }

  it('from the click through stopping, stopped and starting until running: same tabs and chart areas, covered, then uncovered', async () => {
    const c = await openRunning();
    const before = snapshot();
    expect(rxOf(before.panels[2])).toEqual(Array(5).fill(2.5));
    expectKept(before, false, 'running');
    fireEvent.click(restartBtn());
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expectKept(before, true, 'restart clicked');
    // the controller drains: its stats are empty, the state stays `stopping`
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await through(before, true, 'stopping', 4);
    // the stop is done; the page then sends the start
    c.state = 'stopped';
    await through(before, true, 'stopped, start pending', 2);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(c.state).toBe('starting');
    await through(before, true, 'starting', 4);
    for (const d of [1, 2]) expect(rxOf(before.panels[d]), `previous content of panel ${d}`).toHaveLength(5);
    expect(tab(1)).toHaveAttribute('aria-selected', 'true');
    // up again
    c.state = 'running';
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
    expect(block()).toBeNull();
    expect(tabs().map((t) => t.textContent)).toEqual(TITLES);
  });

  it('a start refused after the stop (409) removes the overlay and shows the error', async () => {
    const c = await openRunning();
    const before = snapshot();
    fireEvent.click(restartBtn());
    await tick(0);
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await through(before, true, 'stopping', 2);
    api.startEmulation.mockRejectedValue(new ApiError(409, 'the controller is busy'));
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('the controller is busy');
    expect(allTabs()).toHaveLength(3);
    expect(before.panels[2].isConnected).toBe(true);
  });

  it('a stop that fails removes the overlay and shows the error, the run goes on', async () => {
    await openRunning();
    const before = snapshot();
    const reply = deferred<EmulationView>();
    api.stopEmulation.mockImplementation(() => reply.promise);
    fireEvent.click(restartBtn());
    await tick(0);
    expectKept(before, true, 'stop pending');
    reply.reject(new ApiError(500, 'cannot stop'));
    await tick(0);
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('cannot stop');
    expectKept(before, false, 'after the failed stop');
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('a restart that cannot happen (no links stored) shows the error and no overlay, nothing is stopped', async () => {
    await openRunning();
    seedStore({ links: [] });
    fireEvent.click(restartBtn());
    await tick(0);
    await tick(1000);
    expect(api.stopEmulation).not.toHaveBeenCalled();
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent(/No links configured/);
  });

  it('a restarted run that ends `failed` removes the overlay and shows the emulation error', async () => {
    const c = await openRunning();
    fireEvent.click(restartBtn());
    await tick(0);
    api.getStats.mockResolvedValue(STOPPED_STATS());
    c.state = 'stopped';
    await tick(2500);
    expect(overlay()).not.toBeNull();
    c.state = 'failed';
    api.getEmulation.mockResolvedValue(fx.emulation('failed', { directions: [], error: 'dpds exited (status 1)' }));
    await tick(2500);
    expect(overlay()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('dpds exited (status 1)');
  });
});

// ---- L4 ----------------------------------------------------------------------------------------------------------------------

describe('spec 54 L4: the loader before the first /api/emulation answer', () => {
  it('shows the non-overlay loader (spinner, "DPDS") and no overlay while nothing has answered', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockImplementation(() => new Promise(() => undefined));
    renderApp('/');
    await tick(0);
    await tick(3000);
    const loader = must(block(), 'the loader');
    expect(within(loader as HTMLElement).getByRole('status', { name: /^Loading/ })).toBeInTheDocument();
    expect(loader.querySelector('.loader-text')).toHaveTextContent(/^DPDS$/);
    expect(overlay()).toBeNull();
    expect(screen.queryByRole('tablist')).not.toBeInTheDocument();
  });

  it('gives way to the dashboard once the first answer arrives', async () => {
    vi.useFakeTimers();
    const first = deferred<EmulationView>();
    api.getEmulation.mockImplementationOnce(() => first.promise);
    renderApp('/');
    await tick(0);
    expect(block()).not.toBeNull();
    first.resolve(fx.emulation('stopped'));
    await tick(0);
    await tick(0);
    expect(block()).toBeNull();
    expect(overlay()).toBeNull();
    expect(allTabs()).toHaveLength(3);
    expect(startBtn()).toBeEnabled();
  });

  it('does not come back when a later poll fails', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValueOnce(fx.emulation('stopped')).mockRejectedValue(new Error('Failed to fetch'));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(block()).toBeNull();
    await tick(3000);
    expect(block()).toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('Failed to fetch');
  });
});
