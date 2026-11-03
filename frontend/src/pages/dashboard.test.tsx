import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState, StatsView } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { deferred, field, renderApp, tick } from '../test/helpers';

const STATES: EmulationState[] = ['stopped', 'starting', 'running', 'stopping', 'failed'];

function twoDirections() {
  return fx.stats([fx.dirStats(0), fx.dirStats(1, { rx_pps: 500, tx_pps: 400, rx_bps: 2.5e6, tx_bps: 1.5e3 })]);
}

describe('Dashboard: emulation state and control', () => {
  it.each(STATES)('shows a state badge for "%s"', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    renderApp('/');
    const badge = await screen.findByTestId('state-badge');
    await waitFor(() => expect(badge).toHaveAttribute('data-state', state));
    expect(badge).toHaveTextContent(new RegExp(state, 'i'));
  });

  const ENABLED: Record<EmulationState, [boolean, boolean]> = {
    stopped: [true, false],
    starting: [false, true],
    running: [false, true],
    stopping: [false, false],
    failed: [true, false],
  };
  it.each(STATES)('Start/Stop enablement in "%s"', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    renderApp('/');
    await waitFor(() => expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', state));
    const [start, stop] = ENABLED[state];
    const startBtn = screen.getByRole('button', { name: /^start$/i });
    const stopBtn = screen.getByRole('button', { name: /^stop$/i });
    if (start) expect(startBtn).toBeEnabled(); else expect(startBtn).toBeDisabled();
    if (stop) expect(stopBtn).toBeEnabled(); else expect(stopBtn).toBeDisabled();
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
    await waitFor(() => expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', 'starting'));
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

  it('shows the API error message when start is refused (409)', async () => {
    const user = userEvent.setup();
    api.startEmulation.mockRejectedValue(new ApiError(409, 'port 0000:17:00.0 is a management NIC'));
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(await screen.findByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a management NIC');
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

  it('shows the error and the log tail when failed', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', {
        error: 'dpds exited during startup (status 1)',
        log_tail: ['EAL: Cannot allocate memory', 'direction 0: cannot create mbuf pool dpds_pool_0'],
      }),
    );
    renderApp('/');
    expect(await screen.findByText(/dpds exited during startup \(status 1\)/)).toBeInTheDocument();
    const tail = screen.getByTestId('log-tail');
    expect(within(tail).getByText('EAL: Cannot allocate memory')).toBeInTheDocument();
    expect(within(tail).getByText('direction 0: cannot create mbuf pool dpds_pool_0')).toBeInTheDocument();
  });

  it('shows no error alert and no log tail while stopped without error', async () => {
    renderApp('/');
    await screen.findByTestId('state-badge');
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

describe('Dashboard: per-direction cards', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('renders one card per link with link and device names', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const c1 = await screen.findByTestId('direction-card-1');
    expect(c0).toHaveTextContent('a-b');
    expect(c0).toHaveTextContent('0000:65:00.0');
    expect(c0).toHaveTextContent('0000:65:00.1');
    expect(c1).toHaveTextContent('b-a');
    expect(screen.queryByTestId('direction-card-2')).not.toBeInTheDocument();
  });

  it('shows rx/tx rates in pps and Gb/s', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'RX rate')).toHaveTextContent(/1\.23 Mpps/);
    expect(field(c0, 'RX rate')).toHaveTextContent(/9\.50 Gb\/s/);
    expect(field(c0, 'TX rate')).toHaveTextContent(/1\.20 Mpps/);
    expect(field(c0, 'TX rate')).toHaveTextContent(/8\.25 Gb\/s/);
  });

  it('scales small rates to smaller units', async () => {
    renderApp('/');
    const c1 = await screen.findByTestId('direction-card-1');
    expect(field(c1, 'RX rate')).toHaveTextContent(/500 pps/);
    expect(field(c1, 'RX rate')).toHaveTextContent(/2\.50 Mb\/s/);
    expect(field(c1, 'TX rate')).toHaveTextContent(/400 pps/);
    expect(field(c1, 'TX rate')).toHaveTextContent(/1\.50 kb\/s/);
  });

  it('shows one drop share per cause', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'Loss drops')).toHaveTextContent(/0\.50\s?%/);
    expect(field(c0, 'Limiter drops')).toHaveTextContent(/1\.25\s?%/);
    expect(field(c0, 'Buffer full drops')).toHaveTextContent(/2\.50\s?%/);
    expect(field(c0, 'NIC TX drops')).toHaveTextContent(/0\.10\s?%/);
    expect(field(c0, 'NIC RX drops')).toHaveTextContent(/0\.75\s?%/);
  });

  it('shows mean delay +- std, lateness and the buffer maximum', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'Mean delay').textContent).toMatch(/1\.00 ms\s*±\s*50\.00 [µμ]s/);
    expect(field(c0, 'Lateness')).toHaveTextContent(/2\.50 [µμ]s/);
    expect(field(c0, 'Lateness')).toHaveTextContent(/12\.00 ms/);
    expect(field(c0, 'Buffer max')).toHaveTextContent(/1234|1,234/);
  });

  it('prints n/a for missing delay values', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { delay_mean_ns: null, delay_std_ns: null, lateness_mean_ns: null, lateness_max_ns: null, buffer_len_max: null })]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'Mean delay')).toHaveTextContent('n/a');
    expect(field(c0, 'Lateness')).toHaveTextContent('n/a');
    expect(field(c0, 'Buffer max')).toHaveTextContent('n/a');
  });

  it('renders zero values as numbers, not as n/a', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { loss_pct: 0, limiter_pct: 0, buffer_full_pct: 0, nic_tx_pct: 0, nic_rx_pct: 0, rx_pps: 0, tx_pps: 0, rx_bps: 0, tx_bps: 0, buffer_len_max: 0 })]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'Loss drops')).toHaveTextContent(/0\.00\s?%/);
    expect(field(c0, 'RX rate')).toHaveTextContent(/0 pps/);
    expect(field(c0, 'Buffer max')).toHaveTextContent('0');
    expect(field(c0, 'Buffer max')).not.toHaveTextContent('n/a');
  });

  it('shows no cards while the stats have no directions', async () => {
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
  });

  it('draws a rate/drop line chart once directions exist', async () => {
    renderApp('/');
    await screen.findByTestId('direction-card-0');
    const charts = await screen.findAllByTestId('chart-line');
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
    await screen.findByTestId('direction-card-0');
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
    expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', state);
    expect(api.getStats).toHaveBeenCalled();
    return screen.queryByTestId('stale-indicator') !== null;
  }

  // The payload the controller really sends for a stopped emulation (no age_ms at all).
  const STOPPED_PAYLOAD: StatsView = { directions: [], ports: [], stale: true };

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
    expect(screen.getByTestId('state-badge')).toHaveAttribute('data-state', 'stopped');
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
    const c0 = await screen.findByTestId('direction-card-0');
    const chart = await within(c0).findByTestId('chart-bar');
    const data = JSON.parse(chart.getAttribute('data-chart') ?? 'null');
    expect(data.datasets[0].data).toEqual([1, 10, 100, 40, 2]);
    expect(data.labels).toHaveLength(5);
    const c1 = await screen.findByTestId('direction-card-1');
    const chart1 = await within(c1).findByTestId('chart-bar');
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
    const c0 = await screen.findByTestId('direction-card-0');
    const chart = await within(c0).findByTestId('chart-bar');
    const data = JSON.parse(chart.getAttribute('data-chart') ?? 'null');
    expect(data.datasets[0].data).toHaveLength(500);
    expect(data.labels).toHaveLength(500);
  });

  it('shows the underflow and overflow counts', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    await within(c0).findByTestId('chart-bar');
    expect(c0).toHaveTextContent(/Underflow\D*3\b/);
    expect(c0).toHaveTextContent(/Overflow\D*7\b/);
  });

  it('hides the chart and does not fetch it for a direction with delay_stats: false', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0),
        fx.dirStats(1, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null, delay_min_ns: null, delay_max_ns: null, lateness_mean_ns: null, lateness_max_ns: null }),
      ]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const c1 = await screen.findByTestId('direction-card-1');
    await within(c0).findByTestId('chart-bar');
    expect(within(c1).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(field(c1, 'Mean delay')).toHaveTextContent('n/a');
    expect(api.getDelayHistogram).not.toHaveBeenCalledWith(1);
  });

  it('survives a rejected histogram request (card stays, chart missing)', async () => {
    api.getDelayHistogram.mockRejectedValue(new ApiError(404, 'no histogram'));
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalled());
    expect(within(c0).queryByTestId('chart-bar')).not.toBeInTheDocument();
    expect(field(c0, 'RX rate')).toHaveTextContent(/Mpps/);
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
    expect(screen.queryByTestId('direction-card-0')).not.toBeInTheDocument();
    await tick(4000);
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
    spy.mockRestore();
  });

  it('stops polling after unmount', async () => {
    const view = renderApp('/');
    await tick(3000);
    view.unmount();
    const counts = [api.getEmulation, api.getStats, api.getDelayHistogram].map((m) => m.mock.calls.length);
    await tick(20000);
    expect([api.getEmulation, api.getStats, api.getDelayHistogram].map((m) => m.mock.calls.length)).toEqual(counts);
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

describe('Dashboard: delay min/max and first loss (F2)', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
  });

  it('shows the delay minimum and maximum in their own units', async () => {
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const dd = field(c0, 'Delay min / max');
    expect(dd.textContent).toMatch(/800\.00 [µμ]s\s*\/\s*1\.30 ms/);
  });

  it('prints n/a for the delay minimum/maximum when they are null (delay_stats: false)', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null, delay_min_ns: null, delay_max_ns: null })]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(field(c0, 'Delay min / max')).toHaveTextContent('n/a');
  });

  it('shows the first loss: reason, detail and the time relative to the start', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, {
          first_loss_reason: 'LimiterDrop',
          first_loss_detail: 'rate exceeded by 17 packets',
          first_loss_relative_ns: 2_500_000_000,
        }),
        fx.dirStats(1),
      ]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const dd = field(c0, 'First loss');
    expect(dd).toHaveTextContent('LimiterDrop');
    expect(dd).toHaveTextContent('rate exceeded by 17 packets');
    expect(dd).toHaveTextContent(/2\.50 s/);
  });

  it('a first loss without detail still shows reason and time (no "null")', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0, { first_loss_reason: 'BufferFull', first_loss_detail: null, first_loss_relative_ns: 750_000 })]),
    );
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const dd = field(c0, 'First loss');
    expect(dd).toHaveTextContent('BufferFull');
    expect(dd).toHaveTextContent(/750\.00 [µμ]s/);
    expect(dd).not.toHaveTextContent(/null|undefined/);
  });

  it('prints n/a as first loss while nothing was lost yet', async () => {
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    const c1 = await screen.findByTestId('direction-card-1');
    expect(field(c1, 'First loss')).toHaveTextContent('n/a');
  });
});

describe('Dashboard: NIC ports table (F2)', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
  });

  const COLUMNS = ['ipackets', 'opackets', 'ibytes', 'obytes', 'imissed', 'ierrors', 'oerrors', 'rx_nombuf'];

  /** Text of the cell under `column` in the row whose first cell contains `rowText`. */
  function cell(table: HTMLElement, rowText: string, column: string): string {
    const headers = within(table).getAllByRole('columnheader').map((h) => h.textContent?.trim());
    const col = headers.indexOf(column);
    if (col < 0) throw new Error(`no column ${column} in [${headers.join(', ')}]`);
    // The first cell (a <td> or a <th scope=row>) names the port; header cells and data cells index alike.
    const row = within(table).getAllByRole('row').find((r) => r.children[0]?.textContent?.includes(rowText));
    if (!row) throw new Error(`no row for ${rowText}`);
    return (row.children[col]?.textContent ?? '').trim();
  }
  const digits = (t: string) => t.replace(/[\s,.  ]/g, '');

  it('has one row per port with every counter in its column', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0)], {
        ports: [
          fx.portStats(0),
          fx.portStats(1, { name: '0000:65:00.1', ipackets: 5, opackets: 6, ibytes: 7, obytes: 8, imissed: 9, ierrors: 10, oerrors: 12, rx_nombuf: 13 }),
        ],
      }),
    );
    renderApp('/');
    const table = await screen.findByTestId('ports-table');
    const headers = within(table).getAllByRole('columnheader').map((h) => h.textContent?.trim());
    for (const c of COLUMNS) expect(headers).toContain(c);
    expect(within(table).getAllByRole('row')).toHaveLength(3); // header + 2 ports
    const expected0: Record<string, string> = {
      ipackets: '1234567', opackets: '7654321', ibytes: '98765432100', obytes: '12345678900',
      imissed: '11', ierrors: '22', oerrors: '33', rx_nombuf: '44',
    };
    for (const c of COLUMNS) expect(digits(cell(table, '0000:65:00.0', c))).toBe(expected0[c]);
    const expected1: Record<string, string> = {
      ipackets: '5', opackets: '6', ibytes: '7', obytes: '8', imissed: '9', ierrors: '10', oerrors: '12', rx_nombuf: '13',
    };
    for (const c of COLUMNS) expect(digits(cell(table, '0000:65:00.1', c))).toBe(expected1[c]);
  });

  it('shows zero counters as 0, not as n/a or empty', async () => {
    api.getStats.mockResolvedValue(
      fx.stats([fx.dirStats(0)], { ports: [fx.portStats(0, { imissed: 0, ierrors: 0, oerrors: 0, rx_nombuf: 0 })] }),
    );
    renderApp('/');
    const table = await screen.findByTestId('ports-table');
    for (const c of ['imissed', 'ierrors', 'oerrors', 'rx_nombuf']) expect(cell(table, '0000:65:00.0', c)).toBe('0');
  });

  it('falls back to the port number when the port has no name', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { ports: [fx.portStats(7, { name: null })] }));
    renderApp('/');
    const table = await screen.findByTestId('ports-table');
    const rows = within(table).getAllByRole('row');
    expect(rows).toHaveLength(2);
    expect(rows[1].textContent).toMatch(/\b7\b/);
  });

  it('is not rendered without ports', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { ports: [] }));
    renderApp('/');
    await screen.findByTestId('direction-card-0');
    expect(screen.queryByTestId('ports-table')).not.toBeInTheDocument();
  });

  it('has a heading naming the table (NIC ports)', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { ports: [fx.portStats(0)] }));
    renderApp('/');
    await screen.findByTestId('ports-table');
    expect(screen.getByRole('heading', { name: /NIC ports/i })).toBeInTheDocument();
  });
});

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
    expect(screen.getByTestId('direction-card-0')).toBeInTheDocument();
    spy.mockRestore();
  });

  it('no warning while the stats polls succeed', async () => {
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    await screen.findByTestId('direction-card-0');
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
    const c0 = await screen.findByTestId('direction-card-0');
    const c1 = await screen.findByTestId('direction-card-1');
    const note = await within(c0).findByTestId('histogram-error');
    expect(note).toHaveTextContent(/histogram/i);
    expect(note).toHaveTextContent('histogram read failed');
    expect(within(c0).queryByTestId('chart-bar')).not.toBeInTheDocument();
    // the other direction is unaffected
    await within(c1).findByTestId('chart-bar');
    expect(within(c1).queryByTestId('histogram-error')).not.toBeInTheDocument();
  });

  it('the histogram note disappears and the chart returns when a later fetch succeeds', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    api.getDelayHistogram.mockRejectedValueOnce(new ApiError(500, 'transient')).mockImplementation(async (dir) => fx.histogram(dir));
    renderApp('/');
    await tick(0);
    await tick(0);
    const c0 = screen.getByTestId('direction-card-0');
    expect(within(c0).getByTestId('histogram-error')).toBeInTheDocument();
    await tick(2500);
    expect(within(c0).queryByTestId('histogram-error')).not.toBeInTheDocument();
    expect(within(c0).getByTestId('chart-bar')).toBeInTheDocument();
  });

  it('no histogram note for a direction with delay_stats: false (nothing was fetched)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { delay_stats: false, delay_mean_ns: null, delay_std_ns: null })]));
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    await waitFor(() => expect(api.getStats).toHaveBeenCalled());
    expect(within(c0).queryByTestId('histogram-error')).not.toBeInTheDocument();
  });
});

describe('Dashboard: accessibility and labels (F4)', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('every chart sits in a role="img" element with an aria-label (rate, drops, histogram)', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    await within(c0).findByTestId('chart-bar');
    const imgs = within(c0).getAllByRole('img');
    expect(imgs.length).toBeGreaterThanOrEqual(3);
    for (const img of imgs) expect(img.getAttribute('aria-label')?.trim()).toBeTruthy();
    const names = imgs.map((i) => i.getAttribute('aria-label') ?? '');
    expect(names.some((n) => /rate/i.test(n))).toBe(true);
    expect(names.some((n) => /drop/i.test(n))).toBe(true);
    expect(names.some((n) => /histogram/i.test(n))).toBe(true);
    // the chart canvases are the content of those images
    for (const id of ['chart-line', 'chart-bar']) {
      for (const chart of within(c0).getAllByTestId(id)) expect(chart.closest('[role="img"]')).not.toBeNull();
    }
  });

  it('the chart labels tell the directions apart', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const c1 = await screen.findByTestId('direction-card-1');
    const label = (c: HTMLElement, re: RegExp) =>
      within(c).getAllByRole('img').map((i) => i.getAttribute('aria-label') ?? '').find((n) => re.test(n)) ?? '';
    expect(label(c0, /rate/i)).not.toBe(label(c1, /rate/i));
  });

  it('the emulation error is announced as role="alert"', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', { error: 'dpds exited during startup (status 1)', log_tail: ['EAL: Cannot allocate memory'] }),
    );
    api.getStats.mockResolvedValue(fx.stats([]));
    renderApp('/');
    expect(await screen.findByRole('alert')).toHaveTextContent('dpds exited during startup (status 1)');
  });

  it('the state badge is a polite live region', async () => {
    renderApp('/');
    const badge = await screen.findByTestId('state-badge');
    expect(badge.closest('[aria-live="polite"], [role="status"]')).not.toBeNull();
  });

  it('cards are titled "<link>: <ingress> -> <egress>"', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    const c1 = await screen.findByTestId('direction-card-1');
    expect(within(c0).getByText('a-b: 0000:65:00.0 → 0000:65:00.1')).toBeInTheDocument();
    expect(within(c1).getByText('b-a: 0000:65:00.1 → 0000:65:00.0')).toBeInTheDocument();
  });

  it('the old "Direction d: rx -> tx" title is gone', async () => {
    renderApp('/');
    const c0 = await screen.findByTestId('direction-card-0');
    expect(c0).not.toHaveTextContent(/Direction \d/);
    expect(screen.queryByText(/Direction \d:/)).not.toBeInTheDocument();
  });

  it('a hairpin link reads "loop: net_null0 -> net_null0"', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { link: 'loop', rx: 'net_null0', tx: 'net_null0' })]));
    renderApp('/');
    const card = await screen.findByTestId('direction-card-0');
    expect(within(card).getByText('loop: net_null0 → net_null0')).toBeInTheDocument();
  });

  it('the title uses the link name of the stats entry, ingress as rx and egress as tx (ring of three links)', async () => {
    api.getStats.mockResolvedValue(fx.stats([
      fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
      fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
      fx.dirStats(2, { link: 'c-to-a', rx: 'net_null2', tx: 'net_null0' }),
    ]));
    renderApp('/');
    expect(await screen.findByText('a-to-b: net_null0 → net_null1')).toBeInTheDocument();
    expect(screen.getByText('b-to-c: net_null1 → net_null2')).toBeInTheDocument();
    expect(screen.getByText('c-to-a: net_null2 → net_null0')).toBeInTheDocument();
    expect(screen.getByTestId('direction-card-2')).toHaveTextContent('c-to-a: net_null2 → net_null0');
  });

  it('API calls still use the direction index, not the link name', async () => {
    api.getStats.mockResolvedValue(fx.stats([
      fx.dirStats(0, { link: 'a-to-b', rx: 'net_null0', tx: 'net_null1' }),
      fx.dirStats(1, { link: 'b-to-c', rx: 'net_null1', tx: 'net_null2' }),
    ]));
    renderApp('/');
    await screen.findByText('b-to-c: net_null1 → net_null2');
    await waitFor(() => expect(api.getDelayHistogram).toHaveBeenCalledWith(1));
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);
    for (const call of api.getDelayHistogram.mock.calls) expect(typeof call[0]).toBe('number');
  });
});
