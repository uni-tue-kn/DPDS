import { fireEvent, screen, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { EmulationView } from '../api/types';
import { propsOf } from '../test/chartProps';
import { panelOf, pillText, runButton, summaryTab, tab, tabs } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, renderApp, seedStore, tick } from '../test/helpers';
import { ApiError, api, fakeController, type HistoryServer, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 60 (user 2026-11-06: "DPDS still re-draws the diagrams completely. P4TG does the spinning wheel and just clears the diagram
// content."). Spec: .git/claude-specs/spec60-charts-start-stop.md.
//   R1 Start / Restart click: the overlay shows and, in the same render, every chart is cleared (no samples, zero tables, grey donuts,
//      dash pills); the old run does not come back (also not when a Restart's stop reports `stopped`).
//   R2 the chart elements (Summary, link tabs) stay the same elements from the click through `starting` into `running`; without
//      reported directions the tabs are the stored links.
//   R3 running: same elements, new data.   R4 Stop: no overlay, no clearing, same elements.
//   R5 a failed start removes the overlay, shows the error, the charts stay cleared.   R6 page load: unchanged.
// CHOICES (spec silent): "cleared" tables are either absent or show zeros (every body cell 0, 0 Bit/s or 0.00 %); the controller is
// simulated as the real one: while a run starts it keeps serving the old run's series until the new run (new run_id) exists.

const LINKS = [fx.link(), fx.linkRev()];
const TITLES = ['a-b: enp101s0f0 → enp101s0f1', 'b-a: enp101s0f1 → enp101s0f0'];
const STOPPED_STATS = () => fx.stats([], { stale: true, age_ms: undefined });
const LIVE_STATS = () => fx.stats([fx.dirStats(0), fx.dirStats(1)]);
const ZERO_CELL = /^0(\.0+)?( Bit\/s| %)?$/;
const DASH = '–';

const overlay = () => document.querySelector('.loader-overlay');
const startBtn = () => screen.getByRole('button', { name: /^start$/i });
const restartBtn = () => screen.getByRole('button', { name: /^restart$/i });

interface LineData {
  labels: string[];
  datasets: { label: string; data: number[] }[];
}
const lineData = (el: Element) => propsOf(el).data as LineData;
const rxOf = (el: Element): number[] =>
  must(
    lineData(el).datasets.find((s) => s.label === 'RX rate'),
    'RX dataset',
  ).data;
const donutLabels = (el: Element) => (propsOf(el).data as { labels: string[] }).labels;

/** The tabpanels of the Summary, link 0 and link 1 (every pane stays mounted, so the hidden ones are reachable). */
const panels = () => [panelOf(summaryTab()), panelOf(tab(0)), panelOf(tab(1))];
const lineOf = (panel: HTMLElement) => within(panel).getByTestId('chart-line');
/** The rate chart elements of the Summary, link 0 and link 1. */
const lines = () => panels().map(lineOf);
const donutsOf = (panel: HTMLElement) => Array.from(panel.querySelectorAll('[data-testid="chart-doughnut"]'));

/** Every chart, table, donut of the three views shows no run. */
function expectCleared(what: string) {
  for (const [i, panel] of panels().entries()) {
    const at = `${what}: view ${i}`;
    const data = lineData(lineOf(panel));
    expect(data.labels, `${at}: labels`).toEqual([]);
    for (const s of data.datasets) expect(s.data, `${at}: ${s.label}`).toEqual([]);
    for (const td of Array.from(panel.querySelectorAll('tbody td'))) {
      expect(td.textContent?.trim(), `${at}: table cell`).toMatch(ZERO_CELL);
    }
    const donuts = donutsOf(panel);
    expect(donuts, `${at}: donuts`).toHaveLength(2);
    for (const d of donuts) expect(donutLabels(d), `${at}: donut is the grey placeholder`).toHaveLength(1);
  }
}

/** The three views show the run served by `serveRun()`: five samples, drops in the donuts. */
function expectShowsRun(what: string) {
  for (const [i, panel] of panels().entries()) {
    const at = `${what}: view ${i}`;
    expect(rxOf(lineOf(panel)), `${at}: rx samples`).toHaveLength(5);
    expect(donutLabels(must(donutsOf(panel)[0], 'loss donut')).length, `${at}: loss donut`).toBeGreaterThan(1);
  }
}

const expectPillsDash = (what: string) => {
  expect(pillText('pill-tx'), `${what}: TX pill`).toBe(DASH);
  expect(pillText('pill-rx'), `${what}: RX pill`).toBe(DASH);
};

/** The chart elements and tabs of `before` are still the ones on the page. */
function expectSame(before: Element[], what: string) {
  const now = lines();
  expect(now, `${what}: chart count`).toHaveLength(before.length);
  for (const [i, el] of before.entries()) {
    expect(now[i], `${what}: chart ${i} is the same element`).toBe(el);
    expect(el.isConnected, `${what}: chart ${i} in the document`).toBe(true);
  }
  expect(
    tabs().map((t) => t.textContent),
    `${what}: titles`,
  ).toEqual(TITLES);
}

/** The controller's series: run 1, five samples per link, with its final counters. */
function serveRun(): HistoryServer {
  const s: HistoryServer = {
    startedAt: 1762000000,
    samples: {
      '0': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 1.5 }),
      '1': fx.historySamples(0, 5, { ...fx.NO_PPS, rx_gbps: 2.5 }),
    },
    totals: { '0': fx.totals(), '1': fx.totals() },
  };
  serveHistory(s);
  return s;
}

/** The controller has begun run 2: no samples yet. */
function beginRun2(s: HistoryServer) {
  s.runId = 2;
  s.startedAt = 1762000900;
  s.samples = {};
  s.totals = {};
}

/** Run 2 has its first two samples. */
function run2Samples(s: HistoryServer) {
  s.samples = {
    '0': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 4.5 }),
    '1': fx.historySamples(0, 2, { ...fx.NO_PPS, rx_gbps: 5.5 }),
  };
}

async function open(initial: 'stopped' | 'running' | 'starting') {
  vi.useFakeTimers();
  const c = fakeController(initial);
  api.getStats.mockResolvedValue(initial === 'running' ? LIVE_STATS() : STOPPED_STATS());
  renderApp('/');
  await tick(0);
  await tick(0);
  await tick(1500); // the history is loaded
  return c;
}

/** Polls `n` times half a second apart, running `check` after each. */
async function polls(n: number, check: (ms: number) => void) {
  for (let i = 1; i <= n; i++) {
    await tick(500);
    check(i * 500);
  }
}

beforeEach(() => {
  seedStore({ links: LINKS });
});

// ---- R1 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R1: Start / Restart clear every chart at once, under the overlay', () => {
  it('Start after an ended run: in the render of the click the charts, tables, donuts and pills are empty, and stay so while it starts', async () => {
    serveRun();
    const c = await open('stopped');
    expectShowsRun('before the click');
    const reply = deferred<EmulationView>();
    api.startEmulation.mockImplementation(() => reply.promise);
    fireEvent.click(startBtn());
    expect(overlay(), 'the overlay').not.toBeNull();
    expectCleared('click, request pending');
    expectPillsDash('click, request pending');
    c.state = 'starting';
    reply.resolve(fx.emulation('starting'));
    await tick(0);
    expectCleared('answered with starting');
    // the controller still serves the old run while the new one starts
    await polls(6, (ms) => {
      expect(overlay(), `+${ms} ms: the overlay`).not.toBeNull();
      expectCleared(`starting +${ms} ms`);
      expectPillsDash(`starting +${ms} ms`);
    });
  });

  it('Restart: cleared in the render of the click, and through stopping, `stopped` and starting', async () => {
    serveRun();
    const c = await open('running');
    expectShowsRun('before the click');
    fireEvent.click(restartBtn());
    expect(overlay(), 'the overlay').not.toBeNull();
    expectCleared('click');
    expectPillsDash('click');
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expectCleared('stop answered');
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await polls(3, (ms) => {
      expectCleared(`stopping +${ms} ms`);
      expectPillsDash(`stopping +${ms} ms`);
    });
    // a poll reports `stopped`: the old run must not come back, the start follows
    c.state = 'stopped';
    await polls(6, (ms) => {
      expectCleared(`stopped, then starting +${ms} ms`);
      expectPillsDash(`stopped, then starting +${ms} ms`);
    });
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(c.state).toBe('starting');
  });
});

// ---- R2 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R2: the chart elements stay mounted from the click into the run', () => {
  it('Start after an ended run: same Summary and link charts before the click, under the overlay, starting, running without directions and with data', async () => {
    const s = serveRun();
    const c = await open('stopped');
    const before = lines();
    fireEvent.click(startBtn());
    await tick(0);
    expect(c.state).toBe('starting');
    expectSame(before, 'after the click');
    await polls(3, (ms) => expectSame(before, `starting +${ms} ms`));
    // running, but no direction reported yet: the tabs are the stored links
    beginRun2(s);
    c.state = 'running';
    await polls(4, (ms) => expectSame(before, `running, no directions +${ms} ms`));
    // the run reports data
    run2Samples(s);
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay(), 'no overlay once running').toBeNull();
    expectSame(before, 'running with data');
    expect(rxOf(must(before[1]))).toEqual([4.5, 4.5]);
    expect(rxOf(must(before[2]))).toEqual([5.5, 5.5]);
    expect(rxOf(must(before[0]))).toEqual([10, 10]);
  });

  it('Start on a page without any run: same elements, the stored links as tabs while no direction is reported', async () => {
    const s: HistoryServer = { startedAt: null, samples: {} };
    serveHistory(s);
    const c = await open('stopped');
    const before = lines();
    expect(rxOf(must(before[1]))).toEqual([]);
    fireEvent.click(startBtn());
    await tick(0);
    expectSame(before, 'after the click');
    await polls(3, (ms) => expectSame(before, `starting +${ms} ms`));
    c.state = 'running';
    await polls(3, (ms) => expectSame(before, `running, no directions +${ms} ms`));
    s.startedAt = 1762000900;
    s.runId = 2;
    run2Samples(s);
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay()).toBeNull();
    expectSame(before, 'running with data');
    expect(rxOf(must(before[1]))).toEqual([4.5, 4.5]);
  });

  it('Restart: same elements through stopping, `stopped`, starting, running without directions and with data', async () => {
    const s = serveRun();
    const c = await open('running');
    const before = lines();
    fireEvent.click(restartBtn());
    await tick(0);
    expectSame(before, 'restart clicked');
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await polls(3, (ms) => expectSame(before, `stopping +${ms} ms`));
    c.state = 'stopped';
    await polls(4, (ms) => expectSame(before, `stopped, starting +${ms} ms`));
    expect(c.state).toBe('starting');
    beginRun2(s);
    c.state = 'running';
    await polls(4, (ms) => expectSame(before, `running, no directions +${ms} ms`));
    run2Samples(s);
    api.getStats.mockResolvedValue(LIVE_STATS());
    await tick(2500);
    expect(overlay(), 'no overlay once running').toBeNull();
    expectSame(before, 'running with data');
    expect(rxOf(must(before[1]))).toEqual([4.5, 4.5]);
  });

  it('a page opened while the run is starting, with no run and no directions yet, has the Summary and the stored links as tabs', async () => {
    serveHistory({ startedAt: null, samples: {} });
    await open('starting');
    expect(overlay()).not.toBeNull();
    expect(summaryTab()).toBeInTheDocument();
    expect(tabs().map((t) => t.textContent)).toEqual(TITLES);
    expect(lines()).toHaveLength(3);
  });
});

// ---- R3 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R3: a running run updates the charts in place', () => {
  it('new samples change the data of the same chart elements', async () => {
    const s = serveRun();
    await open('running');
    const before = lines();
    expect(rxOf(must(before[1]))).toHaveLength(5);
    s.samples = {
      '0': [...s.samples['0'], ...fx.historySamples(5, 2, { ...fx.NO_PPS, rx_gbps: 1.5 })],
      '1': [...s.samples['1'], ...fx.historySamples(5, 2, { ...fx.NO_PPS, rx_gbps: 2.5 })],
    };
    await tick(2500);
    expectSame(before, 'running');
    expect(rxOf(must(before[1]))).toHaveLength(7);
    expect(rxOf(must(before[2]))).toHaveLength(7);
    expect(rxOf(must(before[0]))).toHaveLength(7);
    expect(overlay()).toBeNull();
  });
});

// ---- R4 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R4: Stop neither covers nor clears the charts', () => {
  it('Draining… (disabled) while stopping, then stopped: no overlay, the same elements, the final charts stay', async () => {
    serveRun();
    const c = await open('running');
    const before = lines();
    expectShowsRun('running');
    fireEvent.click(screen.getByRole('button', { name: /^stop$/i }));
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(overlay(), 'no overlay at the click').toBeNull();
    expectSame(before, 'stop clicked');
    expectShowsRun('stop clicked');
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await polls(3, (ms) => {
      expect(must(runButton('Draining…'), `+${ms} ms: Draining…`)).toBeDisabled();
      expect(overlay(), `+${ms} ms: no overlay`).toBeNull();
      expectSame(before, `stopping +${ms} ms`);
      expectShowsRun(`stopping +${ms} ms`);
    });
    c.state = 'stopped';
    await polls(4, (ms) => {
      expect(overlay(), `+${ms} ms: no overlay`).toBeNull();
      expectSame(before, `stopped +${ms} ms`);
      expectShowsRun(`stopped +${ms} ms`);
    });
    expect(runButton('Start')).toBeEnabled();
  });
});

// ---- R5 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R5: a failed start removes the overlay, shows the error, and the charts stay cleared', () => {
  it('Start rejected by the controller', async () => {
    serveRun();
    await open('stopped');
    const before = lines();
    const reply = deferred<EmulationView>();
    api.startEmulation.mockImplementation(() => reply.promise);
    fireEvent.click(startBtn());
    expect(overlay()).not.toBeNull();
    reply.reject(new ApiError(500, 'the controller exploded'));
    await tick(0);
    expect(overlay(), 'the overlay is gone').toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('the controller exploded');
    expectSame(before, 'after the failure');
    expectCleared('after the failure');
    await polls(4, (ms) => {
      expectCleared(`later +${ms} ms`);
      expect(screen.getByRole('alert')).toHaveTextContent('the controller exploded');
    });
    expect(startBtn()).toBeEnabled();
  });

  it('Restart whose start is rejected after the stop', async () => {
    serveRun();
    const c = await open('running');
    const before = lines();
    fireEvent.click(restartBtn());
    await tick(0);
    api.getStats.mockResolvedValue(STOPPED_STATS());
    await polls(2, () => undefined);
    api.startEmulation.mockRejectedValue(new ApiError(409, 'the controller is busy'));
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(overlay(), 'the overlay is gone').toBeNull();
    expect(screen.getByRole('alert')).toHaveTextContent('the controller is busy');
    expectSame(before, 'after the failure');
    expectCleared('after the failure');
  });
});

// ---- R6 ----------------------------------------------------------------------------------------------------------------------

describe('spec 60 R6: page load is unchanged', () => {
  it('a controller that still has its last run shows its charts, without overlay', async () => {
    serveRun();
    await open('stopped');
    expect(overlay()).toBeNull();
    expect(tabs().map((t) => t.textContent)).toEqual(TITLES);
    expectShowsRun('loaded');
  });

  it('without any run the stored links give tabs with empty charts, without overlay', async () => {
    serveHistory({ startedAt: null, samples: {} });
    await open('stopped');
    expect(overlay()).toBeNull();
    expect(tabs().map((t) => t.textContent)).toEqual(TITLES);
    expectCleared('page load');
  });
});
