import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState } from '../api/types';
import {
  card,
  CAUSES,
  expectState,
  findCard,
  findDelayBar,
  findState,
  lossDonut,
  queryDelayBar,
  queryRadio,
  radio,
  RUN_BUTTONS,
  runButtonNames,
  runButtons,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { deferred, expectStartBody, renderApp, seedRaw, seedStore, storedDoc, tick } from '../test/helpers';
import { setPrefersDark } from '../test/matchMedia';
import { ApiError, api, fakeController, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 28 (user 2026-11-04): the P4TG-like dashboard. Spec: items 1-3 of scratchpad/spec28.md (item 4, server-side history, is
// spec 29 and not tested here). The tester's choices where the spec is silent are marked "CHOICE" and listed in the report:
//   - test ids: pill-tx, pill-rx, pill-status, pill-elapsed, loss-pct (spec 46, dashboardTopRow.test.tsx: P4TG's order and texts; was
//     pill-lost "Lost N frames" of spec 43; there is no state-badge any more)
//   - the rate pills read "<n> Gbit/s (<m> Mpps)" (spec 46: P4TG's formatBits, no label, no padded decimals), e.g. "9.25 Gbit/s (7.31 Mpps)"
//   - radio group named "Chart", radios named "Traffic rates", "Packet loss", "Delay histogram" (spec 43 F5); the last one only with delay_stats on
//   - the loss chart has nine lines (spec 47), one per donut cause, of cumulative counts (spec 43 F1; was two percent datasets)
//   - Restart refuses locally (no stop) when the local document has no links or is unreadable
//   - the histogram controls, percentile list and under/overflow line belong to the histogram view only
// The chart stubs in test/setup.ts expose the chart data as `data-chart` and the options as `data-options`.
//
// Spec 35 (2026-11-05) reworked this file; each changed or removed block says what it was. In short: the button group always
// holds [Restart] [Start|Stop|...] (spec 43 F8: now Stop/Restart, Draining…/Restart or Start, see the Start/Stop/Restart block); the "dropped" pill is always there (spec 38: now the "lost" pill, only while NIC drops > 0); the
// per-card radio group is per-tab ("Drops" is "Loss", "Delay histogram" is second); the histogram has no controls, no percentile
// list and no expected overlay (F4); the NIC ports table moved to the Interfaces page (interfacesPorts.test.tsx).

const STATES: EmulationState[] = ['stopped', 'starting', 'running', 'stopping', 'failed'];

const startBtn = () => screen.queryByRole('button', { name: /^start$/i });
const stopBtn = () => screen.queryByRole('button', { name: /^stop$/i });
const restartBtn = () => screen.queryByRole('button', { name: /^restart$/i });
const drainingBtn = () => screen.queryByRole('button', { name: /^draining…$/i });

async function openIn(state: EmulationState, over: Partial<ReturnType<typeof fx.emulation>> = {}) {
  api.getEmulation.mockResolvedValue(fx.emulation(state, over));
  renderApp('/');
  await findState(state);
}

// Spec 43 F8. Was (spec 28, spec 35 F1): a ButtonGroup of [Restart] and one contextual Start/Stop/"Stopping…" button, Restart
// always present and enabled only while running, "Stopping…" with a spinner. Now like P4TG's Home.tsx: plain Buttons with `mb-1`,
// no group, no spinners; starting/running [Stop (danger, bi-stop-fill)] [Restart (primary, bi-arrow-clockwise)]; stopping the
// same two disabled with "Draining…" for Stop; any other state only [Start (primary, bi-play-circle-fill)].
describe('Dashboard (spec 43 F8): Start / Stop / Restart like P4TG', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  it.each(['stopped', 'failed'] as const)(
    '"%s": only Start is visible, and it is enabled; there is no Restart',
    async (state) => {
      await openIn(state);
      expect(startBtn()).toBeEnabled();
      expect(stopBtn()).not.toBeInTheDocument();
      expect(restartBtn()).not.toBeInTheDocument();
      expect(drainingBtn()).not.toBeInTheDocument();
    },
  );

  it.each(['starting', 'running'] as const)('"%s": Stop (enabled) then Restart; no Start', async (state) => {
    await openIn(state);
    expect(stopBtn()).toBeEnabled();
    expect(restartBtn()).toBeInTheDocument();
    expect(startBtn()).not.toBeInTheDocument();
    expect(drainingBtn()).not.toBeInTheDocument();
    expect(runButtonNames()).toEqual(['Stop', 'Restart']);
  });

  it('"running": Restart is enabled', async () => {
    await openIn('running');
    expect(must(restartBtn())).toBeEnabled();
  });

  it('"stopping": the same two buttons, both disabled, Stop reading "Draining…"; no Start, no spinner', async () => {
    await openIn('stopping');
    expect(runButtonNames()).toEqual(['Draining…', 'Restart']);
    for (const b of runButtons()) expect(b).toBeDisabled();
    expect(stopBtn()).not.toBeInTheDocument();
    expect(startBtn()).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^stopping…$/i })).not.toBeInTheDocument();
  });

  it('every state has exactly the buttons of RUN_BUTTONS', async () => {
    for (const state of STATES) {
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      const view = renderApp('/');
      await findState(state);
      expect(runButtonNames(), state).toEqual(RUN_BUTTONS[state]);
      view.unmount();
    }
  });

  it('the button follows the state: Start turns into Stop and Restart when the poll reports "running"', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(startBtn()).toBeInTheDocument();
    expect(restartBtn()).not.toBeInTheDocument();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    await tick(2500);
    expect(stopBtn()).toBeInTheDocument();
    expect(startBtn()).not.toBeInTheDocument();
    expect(restartBtn()).toBeInTheDocument();
  });

  it('is plain Buttons like P4TG: no ButtonGroup, no spinner, `mb-1`, Stop danger, Restart and Start primary, with icons', async () => {
    await openIn('running');
    const col = screen.getByTestId('run-buttons');
    expect(col.querySelector('.btn-group')).toBeNull();
    expect(within(col).queryByRole('group')).not.toBeInTheDocument();
    expect(col.querySelector('.spinner-border, .spinner-grow')).toBeNull();
    const stop = must(stopBtn(), 'Stop');
    const restart = must(restartBtn(), 'Restart');
    expect(stop).toHaveClass('btn', 'btn-danger', 'mb-1');
    expect(restart).toHaveClass('btn', 'btn-primary', 'mb-1');
    expect(stop.querySelector('i')).toHaveClass('bi', 'bi-stop-fill');
    expect(restart.querySelector('i')).toHaveClass('bi', 'bi-arrow-clockwise');
    expect(stop.compareDocumentPosition(restart) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(restart).not.toHaveClass('btn-outline-primary');
    for (const b of [stop, restart]) expect(b.style.minWidth).toBe('');
  });

  it('Start is a primary Button with the bi-play-circle-fill icon, `mb-1`, not green', async () => {
    await openIn('stopped');
    const start = must(startBtn(), 'Start');
    expect(start).toHaveClass('btn', 'btn-primary', 'mb-1');
    expect(start).not.toHaveClass('btn-success');
    expect(start.querySelector('i')).toHaveClass('bi', 'bi-play-circle-fill');
    expect(start.style.minWidth).toBe('');
    expect(screen.getByTestId('run-buttons').querySelector('.btn-group, .spinner-border')).toBeNull();
  });

  it('the buttons sit in the right column of the top row, `col-4 text-end`', async () => {
    await openIn('running');
    const col = must(screen.getByTestId('run-buttons').closest('.col-4'), 'the col-4 column');
    expect(col).toHaveClass('text-end');
  });

  it('while a request is in flight (Stop clicked, no answer yet) the buttons are disabled', async () => {
    const user = userEvent.setup();
    api.stopEmulation.mockReturnValue(deferred<ReturnType<typeof fx.emulation>>().promise);
    await openIn('running');
    await user.click(must(stopBtn()));
    await waitFor(() => {
      for (const b of runButtons()) expect(b).toBeDisabled();
    });
    expect(screen.getByTestId('run-buttons').querySelector('.spinner-border')).toBeNull();
  });

  it('Start still sends the whole local document (unchanged by the restyle)', async () => {
    const user = userEvent.setup();
    await openIn('stopped');
    await user.click(must(startBtn()));
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
  });
});

describe('Dashboard (spec 28): Restart = stop, then start with the current local document', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  // Spec 28b item 1: the real controller answers a stop with `stopping` and refuses a start (409) until a later poll reports
  // `stopped` or `failed`. These tests used a stop that resolved with `stopped` at once, which the controller never does:
  // they now run against `fakeController` (src/test/mockApi.ts) and finish the stop by setting its state, under fake timers.
  const openRunning = async () => {
    vi.useFakeTimers();
    const c = fakeController('running');
    renderApp('/');
    await tick(0);
    await tick(0);
    return c;
  };

  it('stops first and starts only after the stop has finished, with the local document', async () => {
    const c = await openRunning();
    fireEvent.click(must(restartBtn()));
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    await tick(3000); // the controller still says "stopping": nothing may happen
    expect(api.startEmulation).not.toHaveBeenCalled();
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(api.stopEmulation.mock.invocationCallOrder[0]).toBeLessThan(api.startEmulation.mock.invocationCallOrder[0]);
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
  });

  it('uses the document as it is stored now: an edit made before the click is included', async () => {
    const c = await openRunning();
    seedStore({ links: [fx.loopLink({ id: 9 })] });
    fireEvent.click(must(restartBtn()));
    await tick(0);
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    const body = api.startEmulation.mock.calls[0][0];
    expect(body.links.map((l) => l.name)).toEqual(['loop']);
  });

  it('does not write the stored document', async () => {
    const c = await openRunning();
    const before = localStorage.getItem('dpds.config.v1');
    fireEvent.click(must(restartBtn()));
    await tick(0);
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalled();
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
  });

  it('a failing stop shows its message, starts nothing and leaves Restart usable', async () => {
    const user = userEvent.setup();
    api.stopEmulation.mockRejectedValue(new ApiError(500, 'cannot signal child'));
    await openIn('running');
    await user.click(must(restartBtn()));
    expect(await screen.findByRole('alert')).toHaveTextContent('cannot signal child');
    expect(api.startEmulation).not.toHaveBeenCalled();
    await waitFor(() => expect(restartBtn()).toBeEnabled());
  });

  it('a refused start after the stop shows the controller message', async () => {
    const c = await openRunning();
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    fireEvent.click(must(restartBtn()));
    await tick(0);
    c.state = 'stopped';
    await tick(2500);
    expect(screen.getByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a host uplink');
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
  });

  it('while the restart is in flight every button is disabled and a second click sends no second stop', async () => {
    vi.useFakeTimers();
    const c = fakeController('running');
    const stopped = deferred<ReturnType<typeof fx.emulation>>();
    api.stopEmulation.mockReturnValue(stopped.promise);
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(restartBtn()));
    await tick(0);
    // spec 43 F8: no "Restarting…" and no spinner any more; while it waits every run button is disabled
    expect(runButtons().length).toBeGreaterThan(0);
    for (const b of runButtons()) expect(b).toBeDisabled();
    expect(screen.getByTestId('run-buttons').querySelector('.spinner-border')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Restarting…' })).not.toBeInTheDocument();
    for (const b of runButtons()) fireEvent.click(b);
    await tick(0);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    c.state = 'stopped';
    stopped.resolve(fx.emulation('stopping'));
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
  });

  it('CHOICE: with no local links Restart refuses first: nothing is stopped, an alert says why', async () => {
    const user = userEvent.setup();
    seedStore({ links: [] });
    await openIn('running');
    await user.click(must(restartBtn()));
    expect(await screen.findByRole('alert')).toHaveTextContent(/no links|nothing to start/i);
    expect(api.stopEmulation).not.toHaveBeenCalled();
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('CHOICE: with an unreadable local configuration Restart is disabled (as Start is)', async () => {
    seedRaw('{not json');
    await openIn('running');
    expect(must(restartBtn())).toBeDisabled();
    expect(must(stopBtn())).toBeEnabled();
  });
});

describe('Dashboard (spec 28): status pills', () => {
  const dirs = () =>
    fx.stats([
      fx.dirStats(0, { rx_bps: 6.1e9, tx_bps: 5.05e9, rx_pps: 4.6e6, tx_pps: 3.9e6 }),
      fx.dirStats(1, { rx_bps: 3.15e9, tx_bps: 2.2e9, rx_pps: 2.71e6, tx_pps: 2.41e6 }),
    ]);

  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(dirs());
  });

  it('shows the total RX rate as "Gbit/s (Mpps)", summed over the directions', async () => {
    renderApp('/');
    const pill = await screen.findByTestId('pill-rx');
    await waitFor(() => expect(pill).toHaveTextContent(/9\.25 Gbit\/s \(7\.31 Mpps\)/));
  });

  it('shows the total TX rate the same way', async () => {
    renderApp('/');
    const pill = await screen.findByTestId('pill-tx');
    await waitFor(() => expect(pill).toHaveTextContent(/7\.25 Gbit\/s \(6\.31 Mpps\)/));
  });

  // Spec 46. Was: 'RX and TX are told apart in the pills (RX first)', with the labels "RX" and "TX" in the text. P4TG has no
  // labels (the arrows tell them apart) and TX comes first.
  it('TX and RX are told apart by their arrows, TX first, with no label in the text', async () => {
    renderApp('/');
    const rx = await screen.findByTestId('pill-rx');
    const tx = await screen.findByTestId('pill-tx');
    expect(rx.textContent).not.toMatch(/RX|TX/);
    expect(tx.textContent).not.toMatch(/RX|TX/);
    expect(must(tx.querySelector('i'))).toHaveClass('bi-arrow-up-circle-fill');
    expect(must(rx.querySelector('i'))).toHaveClass('bi-arrow-down-circle-fill');
    expect(tx.compareDocumentPosition(rx) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('follows the next poll', async () => {
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('pill-rx')).toHaveTextContent(/9\.25 Gbit\/s/);
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { rx_bps: 1.5e9, rx_pps: 1.25e6, tx_bps: 1.5e9, tx_pps: 1.25e6 })]),
    );
    await tick(2500);
    expect(screen.getByTestId('pill-rx')).toHaveTextContent(/1\.5 Gbit\/s \(1\.25 Mpps\)/);
  });

  // Spec 43 F3. Was: 'the state pill stays (same test id and live region)'.
  it('there is no state badge left of the RX pill', async () => {
    renderApp('/');
    await screen.findByTestId('pill-rx');
    expect(screen.queryByTestId('state-badge')).not.toBeInTheDocument();
    expect(screen.queryByText(/^running$/i)).not.toBeInTheDocument();
  });

  // Spec 29, second round (user 2026-11-04): relative times like P4TG. The pill shows the `elapsed_ms` of the stats poll
  // (whole milliseconds since the controller started the run, from its own monotonic clock) and the browser's clock is not
  // used at all. Was: "elapsed time from started_at (unix seconds)", where the pill was `Date.now()` minus `started_at`
  // and counted up by itself once a second; its four tests are rewritten below. `started_at` is for display only.
  describe('elapsed time from the polled elapsed_ms (spec 29, no browser clock)', () => {
    const polled = (elapsed_ms?: number, over: Partial<ReturnType<typeof fx.stats>> = {}) =>
      fx.stats([fx.dirStats(0), fx.dirStats(1)], { ...(elapsed_ms === undefined ? {} : { elapsed_ms }), ...over });
    const pill = () => screen.getByTestId('pill-elapsed');

    async function open(elapsed_ms?: number) {
      vi.useFakeTimers();
      api.getStats.mockResolvedValue(polled(elapsed_ms));
      renderApp('/');
      await tick(0);
      await tick(0);
    }

    it('reads "0h 3m 12s" from elapsed_ms 192000', async () => {
      await open(192_000);
      expect(pill()).toHaveTextContent('0h 3m 12s');
    });

    it('whatever the browser clock says: a clock a year off the start time changes nothing', async () => {
      vi.useFakeTimers();
      vi.setSystemTime((1_762_000_000 + 366 * 86_400) * 1000); // a year after started_at
      api.getStats.mockResolvedValue(polled(192_000));
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(pill()).toHaveTextContent('0h 3m 12s');
      vi.setSystemTime(1000); // and the other way round
      await tick(0);
      expect(pill()).toHaveTextContent('0h 3m 12s');
    });

    it('does not count up by itself: with every request held back it stands still however long the page waits', async () => {
      vi.useFakeTimers();
      let n = 0;
      api.getStats.mockImplementation(() =>
        n++ === 0 ? Promise.resolve(polled(192_000)) : new Promise<ReturnType<typeof fx.stats>>(() => undefined),
      );
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(pill()).toHaveTextContent('0h 3m 12s');
      await tick(30_000);
      expect(pill()).toHaveTextContent('0h 3m 12s');
    });

    it('is updated by each stats poll, to the value that poll carries', async () => {
      await open(192_000);
      api.getStats.mockResolvedValue(polled(197_400));
      await tick(2500);
      expect(pill()).toHaveTextContent('0h 3m 17s');
      api.getStats.mockResolvedValue(polled(210_999));
      await tick(2500);
      expect(pill()).toHaveTextContent('0h 3m 30s');
    });

    it.each([
      [3_725_000, '1h 2m 5s'],
      [59_999, '0h 0m 59s'],
      [1000, '0h 0m 1s'],
      [36_000_000, '10h 0m 0s'],
    ])('elapsed_ms %s is "%s"', async (ms, text) => {
      await open(ms);
      expect(pill()).toHaveTextContent(text);
    });

    // Spec 46 item 4. Was: [0, '0h 0m 0s'] "0 is a value, not absent" (and 999 -> '0h 0m 0s'): the duration shows only when > 0.
    it('is not shown at elapsed_ms 0', async () => {
      await open(0);
      expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
    });

    // The API omits the key when there is no run (`elapsed_ms?: number`, never null).
    it.each([undefined])(
      'is not shown while the stats carry no elapsed_ms (%s), also for a running emulation',
      async (v) => {
        await open(v);
        expectState('running');
        expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
      },
    );

    it('does not need started_at: a running emulation without it shows the polled time', async () => {
      vi.useFakeTimers();
      api.getEmulation.mockResolvedValue(fx.emulation('running', { started_at: null }));
      api.getStats.mockResolvedValue(polled(65_000));
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(pill()).toHaveTextContent('0h 1m 5s');
    });

    // Spec 46 item 4. Was: 'CHOICE: after a stop the pill keeps the frozen final time the controller still reports'.
    it.each(['stopped', 'failed'] as const)(
      'after a run ended ("%s") the pill is gone although the controller still reports the time',
      async (state) => {
        vi.useFakeTimers();
        api.getEmulation.mockResolvedValue(fx.emulation(state));
        api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 305_400 }));
        renderApp('/');
        await tick(0);
        await tick(0);
        expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
        await tick(10_000);
        expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
      },
    );

    it('is not shown for a stopped controller that never ran (no elapsed_ms)', async () => {
      vi.useFakeTimers();
      api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
      api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
      renderApp('/');
      await tick(0);
      await tick(0);
      expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
    });

    // Spec 35: DirectionCard.tsx is gone; the scan covers Dashboard.tsx and every page source that draws a chart.
    it('the dashboard sources use no browser clock and no local tick', () => {
      const dir = join(process.cwd(), 'src/pages');
      const files = readdirSync(dir)
        .filter((f) => /\.tsx?$/.test(f) && !/\.test\./.test(f))
        .filter((f) => f === 'Dashboard.tsx' || readFileSync(join(dir, f), 'utf8').includes('react-chartjs-2'))
        .map((f) => join('src/pages', f));
      expect(files).toContain(join('src/pages', 'Dashboard.tsx'));
      for (const file of files) {
        const src = readFileSync(join(process.cwd(), file), 'utf8');
        for (const forbidden of ['Date.now', 'new Date', 'toLocaleTimeString', 'performance.now', 'setInterval']) {
          expect(src, `${file} uses ${forbidden}`).not.toContain(forbidden);
        }
      }
    });
  });
});

// ---- charts -----------------------------------------------------------------------------------------------------------

interface Dataset {
  label?: string;
  data: number[];
  borderColor?: string;
  backgroundColor?: string;
  fill?: unknown;
}
interface ChartData {
  labels: string[];
  datasets: Dataset[];
}
interface AxisOptions {
  beginAtZero?: boolean;
  min?: number;
  type?: string;
  title?: { text?: string; color?: string; display?: boolean };
  ticks?: { color?: string };
  grid?: { color?: string };
}
interface ChartOptions {
  scales?: { x?: AxisOptions; y?: AxisOptions };
  plugins?: {
    legend?: { display?: boolean; position?: string; labels?: { color?: string } };
    title?: { display?: boolean; text?: string | string[]; color?: string };
    tooltip?: { enabled?: boolean };
    annotation?: { annotations?: unknown };
  };
}

const dataOf = (el: HTMLElement): ChartData => JSON.parse(el.getAttribute('data-chart') ?? 'null');
const optsOf = (el: HTMLElement): ChartOptions => JSON.parse(el.getAttribute('data-options') ?? 'null');
const imgName = (el: HTMLElement) => el.closest('[role="img"]')?.getAttribute('aria-label') ?? '';

function twoDirections() {
  return fx.stats([fx.dirStats(0), fx.dirStats(1)]);
}

/**
 * Spec 29: the rate and drops charts are built from the controller's series (`GET /api/stats/history`), no longer from the
 * stats polls. This serves one sample per link with the values of `fx.dirStats()` (RX 9.5 Gbit/s, TX 8.25 Gbit/s, shares
 * 0.5 / 1.25 / 2.5 / 0.1 / 0.75 %), so the chart tests below keep their numbers.
 */
function serveFixtureHistory() {
  serveHistory({ startedAt: 1762000000, samples: { '0': [fx.historySample(1000)], '1': [fx.historySample(1000)] } });
}

async function openCards() {
  renderApp('/');
  await findCard(1);
  card(0);
}

/** Picks a chart in the card's selector and returns the (one) chart element it shows. */
async function showChart(
  user: ReturnType<typeof userEvent.setup>,
  d: number,
  name: 'Traffic rates' | 'Packet loss' | 'Delay histogram',
) {
  await user.click(radio(d, name));
  return name === 'Delay histogram' ? findDelayBar(card(d)) : within(card(d)).findByTestId('chart-line');
}

// Spec 35 F3/F4. Was: 'radio chart selector per link' with a radio group per card (Rate, Drops, Delay histogram) and the
// histogram's own controls. Now: the radios sit in the chart area of each link's tab (Rate, Delay histogram, Loss). Removed
// here: 'each link has its own selection' (the spec only says the selection persists; dashboardSpec35.test.tsx pins that it
// survives polls and a visit to the other tab) and every histogram control (window, log scale, expected overlay, percentile list).
describe("Dashboard (spec 28, spec 35 F3): radio chart selector in each link's tab", () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('every link has a radio group "Chart" with Rate, Delay histogram and Loss, in this order', async () => {
    await openCards();
    for (const d of [0, 1]) {
      const group = within(card(d)).getByRole('radiogroup', { name: /chart/i });
      const all = within(group).getAllByRole('radio');
      expect(all).toHaveLength(3);
      expect(all[0]).toBe(radio(d, 'Traffic rates'));
      expect(all[1]).toBe(radio(d, 'Delay histogram'));
      expect(all[2]).toBe(radio(d, 'Packet loss'));
    }
  });

  it('Rate is the default and the only chart shown', async () => {
    await openCards();
    expect(radio(0, 'Traffic rates')).toBeChecked();
    expect(radio(0, 'Packet loss')).not.toBeChecked();
    expect(radio(0, 'Delay histogram')).not.toBeChecked();
    const lines = within(card(0)).getAllByTestId('chart-line');
    expect(lines).toHaveLength(1);
    expect(imgName(lines[0])).toMatch(/rate/i);
    expect(queryDelayBar(card(0))).not.toBeInTheDocument();
  });

  it('choosing Loss shows only the loss chart', async () => {
    const user = userEvent.setup();
    await openCards();
    const chart = await showChart(user, 0, 'Packet loss');
    expect(radio(0, 'Packet loss')).toBeChecked();
    expect(radio(0, 'Traffic rates')).not.toBeChecked();
    expect(within(card(0)).getAllByTestId('chart-line')).toHaveLength(1);
    expect(imgName(chart)).toMatch(/loss/i);
    expect(queryDelayBar(card(0))).not.toBeInTheDocument();
  });

  it('choosing Delay histogram shows only the histogram, without any control of its own', async () => {
    const user = userEvent.setup();
    await openCards();
    const chart = await showChart(user, 0, 'Delay histogram');
    expect(imgName(chart)).toMatch(/delay histogram/i);
    expect(within(card(0)).queryByTestId('chart-line')).not.toBeInTheDocument();
    expect(within(card(0)).queryByLabelText('Histogram window')).not.toBeInTheDocument();
    expect(within(card(0)).queryByLabelText('Log scale')).not.toBeInTheDocument();
    expect(within(card(0)).queryByLabelText('Show expected distribution')).not.toBeInTheDocument();
    expect(within(card(0)).queryByTestId('percentiles')).not.toBeInTheDocument();
    expect(within(card(0)).queryByRole('checkbox')).not.toBeInTheDocument();
    expect(within(card(0)).queryByRole('combobox')).not.toBeInTheDocument();
  });

  it('switching back to Rate removes the histogram again', async () => {
    const user = userEvent.setup();
    await openCards();
    await showChart(user, 0, 'Delay histogram');
    await showChart(user, 0, 'Traffic rates');
    expect(queryDelayBar(card(0))).not.toBeInTheDocument();
  });

  it('the selection survives the next polls (the chart area is not rebuilt)', async () => {
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Packet loss')); // fireEvent: userEvent waits on timers that are faked here
    await tick(5000);
    expect(radio(0, 'Packet loss')).toBeChecked();
    expect(imgName(within(card(0)).getByTestId('chart-line'))).toMatch(/loss/i);
  });

  it('a link with delay_stats off has no "Delay histogram" choice (and nothing is fetched for it)', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0), fx.dirStats(1, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null })]),
    );
    await openCards();
    expect(queryRadio(1, 'Delay histogram')).not.toBeInTheDocument();
    expect(radio(1, 'Traffic rates')).toBeInTheDocument();
    expect(radio(1, 'Packet loss')).toBeInTheDocument();
    expect(radio(0, 'Delay histogram')).toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 50));
    expect(api.getDelayHistogram).not.toHaveBeenCalledWith(1);
  });
});

describe('Dashboard (spec 28): rate chart in the P4TG look', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    serveFixtureHistory(); // spec 29
  });

  const rate = async () => {
    await openCards();
    return within(card(0)).getByTestId('chart-line');
  };
  const series = (chart: HTMLElement, re: RegExp) =>
    must(
      dataOf(chart).datasets.find((s) => re.test(s.label ?? '')),
      `${re} dataset`,
    );

  it('has an RX and a TX dataset with the values in Gbit/s', async () => {
    const chart = await rate();
    expect(dataOf(chart).datasets).toHaveLength(2);
    expect(series(chart, /RX/).data.at(-1)).toBeCloseTo(9.5, 6);
    expect(series(chart, /TX/).data.at(-1)).toBeCloseTo(8.25, 6);
  });

  it('RX is P4TG blue rgb(53,162,235), TX orange-brown rgb(183,85,40)', async () => {
    const chart = await rate();
    const rx = series(chart, /RX/);
    const tx = series(chart, /TX/);
    expect(rx.borderColor?.replace(/\s/g, '')).toBe('rgb(53,162,235)');
    expect(tx.borderColor?.replace(/\s/g, '')).toBe('rgb(183,85,40)');
    // the fill is a translucent version of the line colour, not another colour
    expect(rx.backgroundColor?.replace(/\s/g, '')).toMatch(/^rgba?\(53,162,235/);
    expect(tx.backgroundColor?.replace(/\s/g, '')).toMatch(/^rgba?\(183,85,40/);
  });

  it('both rate datasets are filled', async () => {
    const chart = await rate();
    for (const s of dataOf(chart).datasets) {
      expect(s.fill, s.label).toBeTruthy();
    }
  });

  it('the y axis has a title naming the unit and starts at zero', async () => {
    const o = optsOf(await rate());
    expect(o.scales?.y?.title?.text).toMatch(/Gbit\/s/);
    expect(o.scales?.y?.beginAtZero === true || o.scales?.y?.min === 0).toBe(true);
  });

  it('the x axis has a title (time)', async () => {
    const o = optsOf(await rate());
    expect(o.scales?.x?.title?.text).toMatch(/time/i);
  });

  it('the legend is on top', async () => {
    const o = optsOf(await rate());
    expect(o.plugins?.legend?.display).not.toBe(false);
    expect(o.plugins?.legend?.position).toBe('top');
  });

  it('hover tooltips stay enabled', async () => {
    const o = optsOf(await rate());
    expect(o.plugins?.tooltip?.enabled).not.toBe(false);
  });

  // Spec 29. Was: "keeps one sample per poll (the 120 s client history is unchanged)", which counted one point per stats poll.
  it('has one point per sample of the series, however often the stats are polled', async () => {
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(5000);
    expect(dataOf(within(card(0)).getByTestId('chart-line')).labels).toHaveLength(1);
    expect(series(within(card(0)).getByTestId('chart-line'), /RX/).data).toHaveLength(1);
  });
});

// Spec 43 F1. Was: two datasets, "Model loss %" (loss_pct) and "Overload drops %" (limiter + buffer full + NIC TX + NIC RX shares).
// Now one line per donut cause, each the cumulative count of that cause at the sample.
describe('Dashboard (spec 43 F1): the loss chart plots cumulative dropped packets per cause', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    serveFixtureHistory(); // spec 29: one sample per link with the default counts 40 / 30 / 20 / 5 / 4 / 1 / 0 / 0 / 0
  });

  const drops = async () => {
    const user = userEvent.setup();
    await openCards();
    return showChart(user, 0, 'Packet loss');
  };

  it('has exactly nine datasets (spec 47), labelled and ordered like the donut causes', async () => {
    const chart = await drops();
    expect(dataOf(chart).datasets.map((s) => s.label)).toEqual([...CAUSES]);
  });

  it('each line is the cumulative count of its cause (the default sample: 40, 30, 20, 5, 4, 1, 0, 0, 0)', async () => {
    const chart = await drops();
    expect(dataOf(chart).datasets.map((s) => s.data.at(-1))).toEqual([40, 30, 20, 5, 4, 1, 0, 0, 0]);
  });

  it.each(fx.DROP_FIELDS.map((f, i) => [f, i] as const))(
    'one cause alone (%s) moves only its own line',
    async (field, i) => {
      const zero = Object.fromEntries(fx.DROP_FIELDS.map((f) => [f, 0]));
      serveHistory({
        startedAt: 1762000000,
        samples: { '0': [fx.historySample(1000, { ...zero, [field]: 3 })], '1': [fx.historySample(1000)] },
      });
      const chart = await drops();
      const want = Array<number>(fx.DROP_FIELDS.length).fill(0);
      want[i] = 3;
      expect(dataOf(chart).datasets.map((s) => s.data.at(-1))).toEqual(want);
    },
  );

  it('the counts are cumulative, not per sample: the points are the counters as they come', async () => {
    serveHistory({
      startedAt: 1762000000,
      samples: {
        '0': [
          fx.historySample(1000, { loss_drops: 5 }),
          fx.historySample(2000, { loss_drops: 9 }),
          fx.historySample(3000, { loss_drops: 9 }),
        ],
        '1': [],
      },
    });
    const chart = await drops();
    expect(dataOf(chart).datasets[0].data).toEqual([5, 9, 9]);
  });

  it('the lines are told apart by colour, and each has the colour of its donut segment', async () => {
    const chart = await drops();
    const colours = dataOf(chart).datasets.map((s) => s.borderColor);
    expect(new Set(colours).size).toBe(9); // spec 47: nine causes
    const donut = JSON.parse(lossDonut(card(0)).getAttribute('data-chart') ?? 'null');
    expect(colours).toEqual(donut.datasets[0].backgroundColor);
  });

  it('y title is "Dropped packets" (no percent), starts at zero; x title is time; legend on top; tooltips on', async () => {
    const o = optsOf(await drops());
    expect(o.scales?.y?.title?.text).toBe('Dropped packets');
    expect(o.scales?.y?.beginAtZero === true || o.scales?.y?.min === 0).toBe(true);
    expect(o.scales?.x?.title?.text).toMatch(/time/i);
    expect(o.plugins?.legend?.position).toBe('top');
    expect(o.plugins?.tooltip?.enabled).not.toBe(false);
  });

  // Spec 29. Was: "collects one point per poll" (a client-side sample per stats poll).
  it('has one point per sample of the series in every dataset, as many as there are labels', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(1, 3), '1': fx.historySamples(1, 3) },
    });
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(radio(0, 'Packet loss')); // fireEvent: userEvent waits on timers that are faked here
    await tick(3000);
    const chart = within(card(0)).getByTestId('chart-line');
    const d = dataOf(chart);
    expect(d.datasets[0].data).toHaveLength(3);
    expect(d.datasets[0].data.length).toBe(d.labels.length);
    expect(d.datasets[1].data.length).toBe(d.labels.length);
  });
});

describe('Dashboard (spec 28): delay histogram chart look and percentile lines', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  const hist = async () => {
    const user = userEvent.setup();
    await openCards();
    return showChart(user, 0, 'Delay histogram');
  };

  /** Every annotation of the chart as a plain object, whether the plugin options hold a record or a list. */
  const annotations = (o: ChartOptions): Record<string, unknown>[] => {
    const a = o.plugins?.annotation?.annotations;
    if (a === undefined || a === null) return [];
    return (Array.isArray(a) ? a : Object.values(a as object)) as Record<string, unknown>[];
  };
  /** The annotations that mention the percentile `name` as the start of a text ("p50", "p50: 1.05 ms"); p99 is not p99.9. */
  const labelled = (o: ChartOptions, name: string) =>
    annotations(o).filter((a) => {
      const text = JSON.stringify(a);
      const re = new RegExp(`"${name.replace('.', '\\.')}(?![.\\d])`);
      return re.test(text);
    });
  const xOf = (a: Record<string, unknown>): number => {
    const x = (a.xMin ?? a.xValue ?? a.value) as unknown;
    if (typeof x !== 'number') throw new Error(`annotation without a numeric x position: ${JSON.stringify(a)}`);
    return x;
  };

  it('draws p50, p90, p99 and p99.9 as annotations in the chart options', async () => {
    const o = optsOf(await hist());
    for (const name of ['p50', 'p90', 'p99', 'p99.9']) {
      expect(labelled(o, name), name).not.toHaveLength(0);
    }
  });

  it('draws nothing else: no annotation without a percentile name', async () => {
    const o = optsOf(await hist());
    const named = new Set(['p50', 'p90', 'p99', 'p99.9'].flatMap((n) => labelled(o, n)));
    expect(annotations(o).filter((a) => !named.has(a))).toEqual([]);
  });

  it('the lines are vertical (xMin = xMax) with numeric category positions', async () => {
    const o = optsOf(await hist());
    for (const name of ['p50', 'p90', 'p99', 'p99.9']) {
      const a = must(labelled(o, name).find((x) => x.type === 'line') ?? labelled(o, name)[0], `${name} annotation`);
      expect(typeof xOf(a)).toBe('number');
      if (a.type === 'line') expect(a.xMax ?? a.xMin).toBe(a.xMin);
    }
  });

  it('they stand where the values fall: bin axis position (value - lo) / bin - 0.5, within 0.6 of a bin', async () => {
    // fixture: lo 0.9 ms, bin 0.1 ms; p50 1.05 ms, p90 1.15 ms, p99 1.25 ms, p99.9 1.29 ms
    const o = optsOf(await hist());
    const expected: [string, number][] = [
      ['p50', 1.0],
      ['p90', 2.0],
      ['p99', 3.0],
      ['p99.9', 3.4],
    ];
    const xs = expected.map(([name]) => xOf(labelled(o, name)[0]));
    expected.forEach(([name, want], i) => {
      expect(Math.abs(xs[i] - want), `${name} at ${xs[i]}`).toBeLessThanOrEqual(0.6);
    });
    for (let i = 1; i < xs.length; i += 1) expect(xs[i]).toBeGreaterThan(xs[i - 1]);
  });

  it('a percentile of the other direction uses its own reply', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, {
        percentiles: { p50: dir === 0 ? 1050000 : 1350000, p90: 1150000, p99: 1250000, p999: 1290000 },
      }),
    );
    const user = userEvent.setup();
    await openCards();
    // A link whose tab is not shown does not poll or render its histogram: read each link's chart while its tab shows.
    const p50 = async (d: number) => xOf(labelled(optsOf(await showChart(user, d, 'Delay histogram')), 'p50')[0]);
    const p50Of0 = await p50(0);
    expect(p50Of0).toBeLessThan(await p50(1));
  });

  it('no line for a percentile without samples (null)', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) =>
      fx.histogram(dir, { percentiles: { p50: null, p90: null, p99: null, p999: null } }),
    );
    expect(annotations(optsOf(await hist()))).toEqual([]);
  });

  it('no line when the reply has no percentiles at all', async () => {
    api.getDelayHistogram.mockImplementation(async (dir) => {
      const h = fx.histogram(dir);
      delete h.percentiles;
      return h;
    });
    expect(annotations(optsOf(await hist()))).toEqual([]);
  });

  // Spec 35 F4. Was: 'the percentile list under the chart stays' (p50..p99.9 as a list, with clamp markers; removed, the dashed
  // annotation lines stay), 'y axis titled ... legend on top' (the legend assertion is dropped: with the expected overlay gone
  // the single series needs none) and 'keeps the expected overlay in the legend and the log toggle' (overlay and log toggle gone).
  it('y axis titled, starts at zero and linear, x axis titled with the delay; tooltips on', async () => {
    const o = optsOf(await hist());
    expect(o.scales?.y?.title?.text).toMatch(/packets|count/i);
    expect(o.scales?.y?.beginAtZero === true || o.scales?.y?.min === 0).toBe(true);
    expect(o.scales?.y?.type ?? 'linear').toBe('linear');
    expect(o.scales?.x?.title?.text).toMatch(/delay/i);
    expect(o.plugins?.tooltip?.enabled).not.toBe(false);
  });

  it('has the bars only: one dataset, no expected overlay, and no percentile list under the chart', async () => {
    const chart = await hist();
    expect(dataOf(chart).datasets).toHaveLength(1);
    expect(dataOf(chart).datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    expect(within(card(0)).queryByTestId('percentiles')).not.toBeInTheDocument();
    expect(within(card(0)).queryByText('p50', { selector: 'dt' })).not.toBeInTheDocument();
  });

  it('the y axis stays linear whatever happens (there is no log toggle any more)', async () => {
    const chart = await hist();
    expect(within(card(0)).queryByLabelText('Log scale')).not.toBeInTheDocument();
    expect(optsOf(chart).scales?.y?.type ?? 'linear').toBe('linear');
  });

  // Spec 36 removes the title line "Mean <d> · Jitter (σ) <d>" of spec 35 F4 (its tests were here); the numbers are in the
  // histogram tables above the chart (histogramTables.test.tsx).
});

describe('Dashboard (spec 28): chart text and grid colours follow dark mode', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  /** Mean of the r, g, b channels in 0..1 (alpha ignored); supports #rgb, #rrggbb(aa), rgb(...), rgba(...). */
  function brightness(css: string | undefined, what: string): number {
    const c = must(css, what).trim().toLowerCase();
    let ch: number[];
    const hex = /^#([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/.exec(c);
    if (hex) {
      let h = hex[1];
      if (h.length === 3) h = [...h].map((x) => x + x).join('');
      ch = [0, 2, 4].map((i) => Number.parseInt(h.slice(i, i + 2), 16));
    } else {
      const m = /^rgba?\(([^)]*)\)$/.exec(c);
      if (!m) throw new Error(`${what}: cannot parse colour ${css}`);
      ch = m[1]
        .split(/[\s,/]+/)
        .filter(Boolean)
        .slice(0, 3)
        .map(Number);
    }
    return ch.reduce((a, b) => a + b, 0) / 3 / 255;
  }

  const textColours = (o: ChartOptions): [string, string | undefined][] => [
    ['x ticks', o.scales?.x?.ticks?.color],
    ['y ticks', o.scales?.y?.ticks?.color],
    ['x title', o.scales?.x?.title?.color],
    ['y title', o.scales?.y?.title?.color],
    ['legend labels', o.plugins?.legend?.labels?.color],
  ];
  const gridColours = (o: ChartOptions): [string, string | undefined][] => [
    ['x grid', o.scales?.x?.grid?.color],
    ['y grid', o.scales?.y?.grid?.color],
  ];

  const VIEWS = ['Traffic rates', 'Packet loss', 'Delay histogram'] as const;

  async function optionsOf(view: (typeof VIEWS)[number]): Promise<ChartOptions> {
    const user = userEvent.setup();
    await openCards();
    return optsOf(await showChart(user, 0, view));
  }

  it.each(VIEWS)('%s chart, dark: light text and light grid lines', async (view) => {
    setPrefersDark(true);
    const o = await optionsOf(view);
    for (const [what, css] of textColours(o)) expect(brightness(css, what), what).toBeGreaterThan(0.6);
    for (const [what, css] of gridColours(o)) expect(brightness(css, what), what).toBeGreaterThan(0.5);
  });

  it.each(VIEWS)('%s chart, light: dark text and dark grid lines', async (view) => {
    setPrefersDark(false);
    const o = await optionsOf(view);
    for (const [what, css] of textColours(o)) expect(brightness(css, what), what).toBeLessThan(0.4);
    for (const [what, css] of gridColours(o)) expect(brightness(css, what), what).toBeLessThan(0.5);
  });

  it('toggling the theme re-colours an open chart without waiting for the next poll', async () => {
    const user = userEvent.setup();
    setPrefersDark(false);
    await openCards();
    const chart = () => within(card(0)).getByTestId('chart-line');
    expect(brightness(optsOf(chart()).scales?.y?.ticks?.color, 'light ticks')).toBeLessThan(0.4);
    await user.click(screen.getByRole('button', { name: 'Toggle dark mode' }));
    await waitFor(() => expect(brightness(optsOf(chart()).scales?.y?.ticks?.color, 'dark ticks')).toBeGreaterThan(0.6));
    await user.click(screen.getByRole('button', { name: 'Toggle dark mode' }));
    await waitFor(() => expect(brightness(optsOf(chart()).scales?.y?.ticks?.color, 'light again')).toBeLessThan(0.4));
  });

  it('the series colours are the same in both themes (RX blue, TX orange-brown)', async () => {
    setPrefersDark(true);
    await openCards();
    const rx = dataOf(within(card(0)).getByTestId('chart-line')).datasets.find((s) => /RX/.test(s.label ?? ''));
    expect(rx?.borderColor?.replace(/\s/g, '')).toBe('rgb(53,162,235)');
  });
});

// Spec 35 F6. Removed: 'PortsTable style' (striped, dark header) and its rows/heading test; the table is on the Interfaces page and
// interfacesPorts.test.tsx pins them there.
