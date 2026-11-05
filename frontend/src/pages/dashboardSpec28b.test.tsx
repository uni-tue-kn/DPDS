import { fireEvent, screen, waitFor } from '@testing-library/react';
import { vi } from 'vitest';
import * as fx from '../test/fixtures';
import { expectStartBody, renderApp, seedStore, storedDoc, tick } from '../test/helpers';
import { ApiError, api, fakeController, serveHistory } from '../test/mockApi';
import { must } from '../test/must';
import { expectState, runButtons } from '../test/dashboardUi';

// Spec 28b: fixes from the review of bcca62b (items 1, 3, 4 and 5 here; item 2 is dashboardMemo.test.tsx, item 6 is
// histogramHintsSpec28b.test.tsx). Spec: scratchpad/spec28b.md. The tester's choices where it is silent are marked "CHOICE".
//
// Spec 43 F8 changed the "Restarting…" block: no such button or spinner any more, the run buttons are just disabled while it waits.
// Spec 35 (2026-11-05) changed this file: Restart was always in the button group (disabled, not absent); the "no RX/TX pills while stopped" block is inverted (the pills stay, "–" before a run, the
// final values after one); the per-card histogram settings block (item 5) is removed with the controls it was about.
//
// Item 1 is about a mismatch between the mocks and the controller: the controller answers `POST /api/emulation/stop` with
// 202 and state `stopping`, and answers a start with 409 until a later `GET /api/emulation` reports `stopped` or `failed`.
// These tests run against `fakeController` (src/test/mockApi.ts), which behaves that way and records, for every start,
// the state the client had last polled.

const startBtn = () => screen.queryByRole('button', { name: /^start$/i });
const stopBtn = () => screen.queryByRole('button', { name: /^stop$/i });
const restartBtn = () => screen.queryByRole('button', { name: /^restart$/i });
const restartingBtn = () => screen.queryByRole('button', { name: 'Restarting…' });

async function openRunning() {
  vi.useFakeTimers();
  const c = fakeController('running');
  const view = renderApp('/');
  await tick(0);
  await tick(0);
  return { c, view };
}

const clickRestart = async () => {
  fireEvent.click(must(restartBtn(), 'the Restart button'));
  await tick(0);
};

describe('Dashboard (spec 28b item 1): Restart waits until the stop has finished', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  // Spec 43 F8. Was: a disabled "Restarting…" button with a spinner and no Start/Stop/"Stopping…" while the restart waits. P4TG
  // has no such state: no spinner and no "Restarting…"; while the restart's request is in flight (busy) the buttons are disabled.
  it('the stop request is sent at once and the run buttons are disabled, without a spinner or "Restarting…"', async () => {
    await openRunning();
    await clickRestart();
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(runButtons().length).toBeGreaterThan(0);
    for (const b of runButtons()) expect(b).toBeDisabled();
    expect(restartingBtn()).not.toBeInTheDocument();
    expect(screen.getByTestId('run-buttons').querySelector('.spinner-border')).toBeNull();
  });

  it('while it waits every run button stays disabled, whatever the polls report, and no Start is offered', async () => {
    const { c } = await openRunning();
    await clickRestart();
    for (const state of ['stopping', 'stopping', 'running'] as const) {
      c.state = state; // "running" = a lagging poll
      await tick(1000);
      expect(runButtons().length, state).toBeGreaterThan(0);
      for (const b of runButtons()) expect(b, state).toBeDisabled();
      expect(startBtn(), state).not.toBeInTheDocument();
      expect(restartingBtn(), state).not.toBeInTheDocument();
    }
  });

  it('nothing is started while the polls say "stopping" (or still "running")', async () => {
    const { c } = await openRunning();
    await clickRestart();
    await tick(4000);
    expect(c.state).toBe('stopping');
    expect(api.startEmulation).not.toHaveBeenCalled();
    c.state = 'running'; // the controller had not noticed the stop yet
    await tick(3000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    c.state = 'stopping';
    await tick(3000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
  });

  it.each(['stopped', 'failed'] as const)(
    'once a poll has reported "%s" the start is sent, after that poll and with no 409 shown',
    async (end) => {
      const { c } = await openRunning();
      await clickRestart();
      await tick(2000);
      c.state = end;
      await tick(2500);
      expect(api.startEmulation).toHaveBeenCalledTimes(1);
      // the client had polled the end state before it sent the start, and the controller accepted it
      expect(c.starts).toEqual([{ state: end, lastPolled: end }]);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(api.stopEmulation.mock.invocationCallOrder[0]).toBeLessThan(
        api.startEmulation.mock.invocationCallOrder[0],
      );
    },
  );

  it('never sends a start that the controller would refuse with 409 (the whole flow, polls as they come)', async () => {
    const { c } = await openRunning();
    await clickRestart();
    await tick(3000);
    c.state = 'stopped';
    await tick(3000);
    await tick(3000);
    expect(c.starts.every((s) => s.state === 'stopped' || s.state === 'failed')).toBe(true);
    expect(c.starts).toHaveLength(1);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('then the buttons of the new run are shown: Stop and Restart, enabled', async () => {
    const { c } = await openRunning();
    await clickRestart();
    c.state = 'stopped';
    await tick(2500);
    c.state = 'running';
    await tick(2500);
    expect(restartingBtn()).not.toBeInTheDocument();
    expect(stopBtn()).toBeEnabled();
    expect(restartBtn()).toBeEnabled();
  });

  it('the start carries the local document as it is stored when the start happens, not when Restart was clicked', async () => {
    const { c } = await openRunning();
    await clickRestart();
    seedStore({ links: [fx.loopLink({ id: 9 })] });
    await tick(2000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
    expect(api.startEmulation.mock.calls[0][0].links.map((l) => l.name)).toEqual(['loop']);
  });

  it('a second click while it waits does nothing: one stop, one start', async () => {
    const { c } = await openRunning();
    await clickRestart();
    fireEvent.click(must(restartBtn(), 'the disabled Restart button'));
    fireEvent.click(must(restartBtn(), 'the disabled Restart button'));
    await tick(0);
    c.state = 'stopped';
    await tick(2500);
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
  });

  it('leaving the dashboard cancels the pending start', async () => {
    const { c, view } = await openRunning();
    await clickRestart();
    await tick(1500);
    view.unmount();
    c.state = 'stopped';
    await tick(10000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
  });

  it('a stop request error cancels the restart: the message is shown, nothing is ever started, Restart is usable again', async () => {
    const { c } = await openRunning();
    api.stopEmulation.mockRejectedValue(new ApiError(500, 'cannot signal child'));
    await clickRestart();
    expect(screen.getByRole('alert')).toHaveTextContent('cannot signal child');
    expect(restartingBtn()).not.toBeInTheDocument();
    expect(restartBtn()).toBeEnabled();
    c.state = 'stopped'; // even if the emulation then happens to stop
    await tick(5000);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('a start refused after the wait shows the controller message and puts Start back (enabled)', async () => {
    const { c } = await openRunning();
    await clickRestart();
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    c.state = 'stopped';
    await tick(2500);
    expect(screen.getByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a host uplink');
    expect(restartingBtn()).not.toBeInTheDocument();
    expect(startBtn()).toBeEnabled();
  });

  it('the pending restart survives polls that fail: it waits for a poll that does report the end', async () => {
    const { c } = await openRunning();
    await clickRestart();
    const polls = api.getEmulation.getMockImplementation();
    api.getEmulation.mockRejectedValue(new Error('Failed to fetch'));
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await tick(3000);
    expect(api.startEmulation).not.toHaveBeenCalled();
    api.getEmulation.mockImplementation(must(polls, 'the polling mock'));
    c.state = 'stopped';
    await tick(2500);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    errors.mockRestore();
  });
});

// ---- item 3 ------------------------------------------------------------------------------------------------------------

// Spec 46. Was: 'the pill unit is chosen after rounding' (formatRatePill: 999 996 bit/s is "1.00 Mbit/s", pps units up to Mpps, two
// decimals everywhere). P4TG's formatBits/formatPacketRate replace it: the unit comes from the magnitude before rounding, trailing
// zeros are dropped, units go up to Tbit/s and Tpps. The formatters are pinned with literals in common/format.test.ts.
describe("Dashboard (spec 28b item 3, spec 46): the pills use P4TG's formatBits and formatPacketRate", () => {
  it('the TX and RX pills read the summed rates in P4TG units', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { rx_bps: 1.5e6, rx_pps: 2500, tx_bps: 999.6, tx_pps: 999.6 }),
        fx.dirStats(1, { rx_bps: 0, rx_pps: 0, tx_bps: 0, tx_pps: 0 }),
      ]),
    );
    renderApp('/');
    const rx = await screen.findByTestId('pill-rx');
    await waitFor(() => expect(rx).toHaveTextContent('1.5 Mbit/s (2.5 Kpps)'));
    expect(screen.getByTestId('pill-tx')).toHaveTextContent('999.6 Bit/s (999.6 Pps)');
  });
});

// ---- item 4 ------------------------------------------------------------------------------------------------------------

describe('Dashboard (spec 28b item 4): the action error and the live poll error', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    vi.useFakeTimers();
  });

  const refusedStart = async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(startBtn(), 'Start'));
    await tick(0);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
  };

  it('an action error stays while the state does not change, however many polls come', async () => {
    await refusedStart();
    await tick(6000);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
  });

  it.each(['starting', 'running', 'failed'] as const)(
    'it disappears when the polled state changes to "%s"',
    async (state) => {
      await refusedStart();
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      await tick(2500);
      expect(screen.queryByText(/host uplink/)).not.toBeInTheDocument();
    },
  );

  it('it disappears when a later action succeeds', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.startEmulation
      .mockRejectedValueOnce(new ApiError(409, 'port 0000:17:00.0 is a host uplink'))
      .mockResolvedValue(fx.emulation('starting'));
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(must(startBtn()));
    await tick(0);
    expect(screen.getByRole('alert')).toHaveTextContent('host uplink');
    fireEvent.click(must(startBtn()));
    await tick(0);
    expect(api.startEmulation).toHaveBeenCalledTimes(2);
    expect(screen.queryByText(/host uplink/)).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('a failed poll is shown next to an old action error: both messages are on the page', async () => {
    await refusedStart();
    api.getEmulation.mockRejectedValue(new Error('Failed to fetch'));
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await tick(2500);
    const alerts = screen.getAllByRole('alert').map((a) => a.textContent ?? '');
    const all = alerts.join(' | ');
    expect(all).toContain('host uplink');
    expect(all).toMatch(/failed to fetch/i);
    errors.mockRestore();
  });

  it('the poll error goes away when polls work again (as before)', async () => {
    api.getEmulation.mockRejectedValue(new Error('Failed to fetch'));
    const errors = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByRole('alert')).toHaveTextContent(/failed to fetch/i);
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    await tick(2500);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    errors.mockRestore();
  });
});

// Spec 35 F2, spec 37. Was: 'no RX/TX pills while stopped' (no pill when the stats have no directions; they went away with the run).
// The pills stay now: "–" while nothing has run, "0 Bit/s (0 Pps)" (spec 46: formatBits(0), formatPacketRate(0)) after it. Full
// contract: dashboardSpec35.test.tsx, dashboardTopRow.test.tsx.
describe('Dashboard (spec 28b item 4, spec 37): RX and TX pills when the stats have no directions', () => {
  it('stopped, nothing ever ran: "–" in both pills, not "0 Bit/s (0 Pps)"', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    await tick(0);
    await tick(0);
    await tick(2500);
    expectState('stopped');
    expect(screen.getByTestId('pill-rx')).toHaveTextContent(/^\s*–$/);
    expect(screen.getByTestId('pill-tx')).toHaveTextContent(/^\s*–$/);
    expect(screen.queryByText(/0 Bit\/s/)).not.toBeInTheDocument();
  });

  it('they show the live numbers while links run and 0 when it has stopped', async () => {
    vi.useFakeTimers();
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': [fx.historySample(1000, { rx_gbps: 4.5, tx_gbps: 4, rx_mpps: 2, tx_mpps: 1.5 })] },
      totals: { '0': fx.totals() },
    });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)])); // 9.5 Gbit/s, 1.23 Mpps
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('pill-rx')).toHaveTextContent('9.5 Gbit/s (1.23 Mpps)');
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 4000 }));
    await tick(2500);
    expect(screen.getByTestId('pill-rx')).toHaveTextContent('0 Bit/s (0 Pps)');
    expect(screen.getByTestId('pill-tx')).toHaveTextContent('0 Bit/s (0 Pps)');
  });

  it('a starting emulation without directions yet shows "–", not empty pills and not the old run', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(screen.getByTestId('pill-rx')).toHaveTextContent(/^\s*–$/);
    expect(screen.getByTestId('pill-tx')).toHaveTextContent(/^\s*–$/);
  });
});

// Spec 35 F4. Removed: 'item 5: histogram settings are kept per card' (window, log scale and expected overlay kept across chart
// switches, per card). The histogram has none of these controls any more (see dashboardP4tg.test.tsx and histogramExtras.test.tsx).
