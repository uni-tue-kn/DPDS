// User flows of the Dashboard page. The charts are stubs that expose their data (test/setup.ts); the controller is the mock API.
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {
  bodyCells,
  card,
  chartIn,
  findCard,
  findState,
  hasChart,
  pillText,
  radioIn,
  runButton,
  runButtonNames,
  summary,
  tabTitles,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { expectStartBody, renderApp, seedStore, storedDoc } from '../test/helpers';
import { ApiError, api, fakeController, type HistoryServer, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

const SLOW = { timeout: 4000 };

const twoDirections = () =>
  fx.stats([
    fx.dirStats(0),
    fx.dirStats(1, {
      delay_stats: false,
      rx_pps: 500,
      tx_pps: 400,
      rx_bps: 2.5e6,
      tx_bps: 1.5e3,
      counters: fx.counters(),
    }),
  ]);

/** The tab titles once the interface list has turned PCI addresses into netdev names. */
const findTitles = async (titles: string[]) => waitFor(() => expect(tabTitles()).toEqual(titles), SLOW);
const TITLES = ['a-b: enp101s0f0 → enp101s0f1', 'b-a: enp101s0f1 → enp101s0f0'];

async function clickWhenEnabled(user: ReturnType<typeof userEvent.setup>, name: string) {
  const button = runButton(name);
  await waitFor(() => expect(button).toBeEnabled());
  await user.click(button);
}

beforeEach(() => {
  seedStore({ links: [fx.link(), fx.linkRev()] });
});

describe('Dashboard: run control', () => {
  it('start sends the stored links, the live statistics show, stop drains and ends the run', async () => {
    const user = userEvent.setup();
    const c = fakeController('stopped');
    api.getStats.mockImplementation(async () => (c.state === 'running' ? twoDirections() : fx.stats([])));
    const run: HistoryServer = { startedAt: null, samples: {} };
    serveHistory(run);
    renderApp('/');
    await findState('stopped');
    expect(pillText('pill-tx')).toBe('–');

    await clickWhenEnabled(user, 'Start');
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
    await findState('starting');

    // the controller reports a new run (a new run_id) once it exists; until then the page shows no run
    c.state = 'running';
    run.startedAt = 1762000000;
    run.samples = { '0': fx.historySamples(0, 2), '1': fx.historySamples(0, 2) };
    await findState('running');
    await waitFor(() => expect(pillText('pill-tx')).toBe('8.44 Gbit/s (1.2 Mpps)'), SLOW);
    expect(pillText('pill-rx')).toContain('Gbit/s');
    await findTitles(TITLES);

    await clickWhenEnabled(user, 'Stop');
    expect(api.stopEmulation).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(runButtonNames()).toEqual(['Draining…', 'Restart']));
    for (const b of within(screen.getByTestId('run-buttons')).getAllByRole('button')) expect(b).toBeDisabled();

    c.state = 'stopped';
    await findState('stopped');
    await waitFor(() => expect(pillText('pill-tx')).toBe('0 Bit/s (0 Pps)'), SLOW);
  });

  it('a refused start shows the error and stays startable', async () => {
    const user = userEvent.setup();
    api.startEmulation.mockRejectedValue(new ApiError(400, 'delayer.std_ns must be >= 0'));
    renderApp('/');
    await findState('stopped');
    await clickWhenEnabled(user, 'Start');
    expect(await screen.findByRole('alert')).toHaveTextContent('delayer.std_ns must be >= 0');
    await waitFor(() => expect(runButton('Start')).toBeEnabled());
  });

  it('without stored links Start sends nothing and says so', async () => {
    const user = userEvent.setup();
    seedStore({ links: [] });
    renderApp('/');
    await findState('stopped');
    await clickWhenEnabled(user, 'Start');
    expect(await screen.findByRole('alert')).toHaveTextContent(/no links/i);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('restart stops first and starts only after a poll has reported the end of the run', async () => {
    const user = userEvent.setup();
    const c = fakeController('running');
    api.getStats.mockResolvedValue(twoDirections());
    renderApp('/');
    await findState('running');
    await clickWhenEnabled(user, 'Restart');
    await waitFor(() => expect(api.stopEmulation).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(c.lastPolled).toBe('stopping'), SLOW);
    expect(api.startEmulation).not.toHaveBeenCalled();

    c.state = 'stopped';
    await waitFor(() => expect(api.startEmulation).toHaveBeenCalledTimes(1), SLOW);
    expect(must(c.starts[0]).lastPolled).toBe('stopped');
  });

  it('a failed run shows the error and the controller log, and can be started again', async () => {
    api.getEmulation.mockResolvedValue(
      fx.emulation('failed', { error: 'dpds exited with 1', log_tail: ['EAL: a', 'EAL: a', 'EAL: b'] }),
    );
    renderApp('/');
    expect(await screen.findByRole('alert')).toHaveTextContent('dpds exited with 1');
    expect(must(screen.getByTestId('log-tail').textContent)).toBe('EAL: aEAL: aEAL: b');
    await findState('failed');
  });
});

describe('Dashboard: statistics and notices', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
  });

  it('computes the pills, the loss share and the per-link tables from the live statistics', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    renderApp('/');
    // 10 dropped frames (loss 4, limiter 3, buffer 3): the status turns to an error
    await waitFor(() => expect(pillText('pill-status')).toMatch(/^Status: Error\. Lost 10\.00 frames\.$/), SLOW);
    // loss share = 1 - TX L2 / RX L2 = 1 - 8.25 / 9.5
    expect(screen.getByTestId('loss-pct')).toHaveTextContent('13.16 %');

    const panel = await findCard(0);
    expect(bodyCells(panel, 'TX L1')).toEqual(['8.44 Gbit/s', '9.7 Gbit/s', '8.25 Gbit/s', '9.5 Gbit/s']);
    expect(bodyCells(panel, 'Lost Frames')).toEqual(['10.00', '1.00 %', '7.00']);
    expect(chartIn(panel, 'Distribution of the dropped packets by cause').data.datasets[0]?.data).toEqual([
      4, 3, 3, 0, 0, 0, 0, 0, 0,
    ]);
    expect(chartIn(panel, 'Distribution of the received frames by size').data.datasets[0]?.data).toEqual(
      fx.counters().frame_sizes,
    );
  });

  it('without drops the status is Ok and there is no loss share', async () => {
    const clean = fx.counters({ loss_drops: 0, limiter_drops: 0, buffer_full_drops: 0 });
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { counters: clean, tx_bps: 9.5e9 })]));
    renderApp('/');
    await waitFor(() => expect(pillText('pill-status')).toBe('Status: Ok'), SLOW);
    expect(screen.queryByTestId('loss-pct')).not.toBeInTheDocument();
  });

  it('the Summary adds up the links; counts that are not measurable (SR-IOV) stay out of the sum', async () => {
    const vf = { nic_rx_port_buffer_discards: null, nic_rx_phy_discards: null, nic_tx_phy_discards: null };
    api.getStats.mockResolvedValue(
      fx.stats([
        fx.dirStats(0, { counters: fx.counters(vf) }),
        fx.dirStats(1, { counters: fx.counters({ ...vf, loss_drops: 6 }) }),
      ]),
    );
    renderApp('/');
    await findTitles(TITLES);
    const panel = summary();
    expect(chartIn(panel, 'Distribution of the dropped packets by cause').data.datasets[0]?.data).toEqual([
      10,
      6,
      6,
      0,
      0,
      0,
      null,
      null,
      null,
    ]);
    expect(bodyCells(panel, 'Lost Frames')[0]).toBe('22.00');
    expect(screen.getByText('Lost 22.00 frames.', { exact: false })).toBeInTheDocument();
  });

  it('the loss donut hover text gives the count and the share of all drops, and n/a for an unmeasurable cause', async () => {
    const vf = { nic_rx_port_buffer_discards: null, nic_rx_phy_discards: null, nic_tx_phy_discards: null };
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0, { counters: fx.counters({ ...vf, loss_drops: 1234 }) })]));
    renderApp('/');
    const panel = await findCard(0);
    const donut = chartIn(panel, 'Distribution of the dropped packets by cause');
    const { data, props } = donut;
    const label = (props.options as { plugins: { tooltip: { callbacks: { label: (c: unknown) => string } } } }).plugins
      .tooltip.callbacks.label;
    const dataset = must(data.datasets[0]);
    const hover = (i: number) => label({ label: must(data.labels[i]), raw: dataset.data[i], dataset });
    // 1234 of 1240 drops
    expect(hover(0)).toBe('Loss model: 1,234 packets (99.52 % of drops)');
    expect(hover(1)).toBe('Rate limiter: 3 packets (0.24 % of drops)');
    expect(hover(6)).toBe('NIC RX port buffer full: n/a');
    expect(within(panel).getAllByText('n/a')).toHaveLength(3);
  });

  it('shows a stale indicator only for a running emulation with an old answer', async () => {
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)], { stale: true, age_ms: 3500 }));
    renderApp('/');
    expect(await screen.findByTestId('stale-indicator', {}, SLOW)).toHaveTextContent('3.5 s');
  });

  it('a failing statistics poll warns with the message, the warning can be closed and the page keeps working', async () => {
    const user = userEvent.setup();
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getStats.mockRejectedValue(new Error('stats are down'));
    renderApp('/');
    const warning = await screen.findByTestId('stats-warning');
    expect(warning).toHaveTextContent('stats are down');
    await user.click(within(warning).getByRole('button'));
    expect(screen.queryByTestId('stats-warning')).not.toBeInTheDocument();
    expect(runButtonNames()).toEqual(['Stop', 'Restart']);
  });

  it('a failing history poll warns with the message', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    api.getStatsHistory.mockRejectedValue(new Error('history is down'));
    renderApp('/');
    expect(await screen.findByTestId('history-warning')).toHaveTextContent('history is down');
  });

  it('an unreachable emulation endpoint shows its error instead of a page that never loads', async () => {
    api.getEmulation.mockRejectedValue(new Error('connection refused'));
    renderApp('/');
    expect(await screen.findByRole('alert')).toHaveTextContent('connection refused');
  });
});

describe('Dashboard: tabs and charts', () => {
  it('before any run there is a tab per stored link, no tables and no histogram', async () => {
    renderApp('/');
    await findTitles(TITLES);
    expect(pillText('pill-rx')).toBe('–');
    const panel = card(0);
    expect(within(panel).queryAllByRole('table')).toHaveLength(0);
    expect(radioIn(panel, 'Delay histogram')).toBeDisabled();
    expect(radioIn(summary(), 'IAT histogram')).toBeDisabled();
  });

  it('the rate and loss charts plot the history in L1 units; the Summary sums the links', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3, { loss_drops: 1 }) },
    });
    renderApp('/');
    const panel = await findCard(0);
    // L1 = L2 + 160 bit per frame: 8.25 Gbit/s + 1.2 Mpps * 160 bit = 8.442 Gbit/s
    await waitFor(() => {
      const tx = chartIn(panel, 'Rate chart of link 0').data.datasets.find((d) => d.label === 'TX rate');
      expect(tx?.data).toHaveLength(3);
      expect(must(tx?.data[0])).toBeCloseTo(8.442, 6);
    }, SLOW);
    const sum = chartIn(summary(), 'Rate chart of all links').data.datasets.find((d) => d.label === 'TX rate');
    expect(must(sum?.data[0])).toBeCloseTo(2 * 8.442, 6);

    radioIn(card(0), 'Packet loss').click();
    await waitFor(() => expect(hasChart(card(0), 'Loss chart of link 0')).toBe(true));
    const loss = chartIn(card(0), 'Loss chart of link 0').data.datasets;
    expect(loss).toHaveLength(9);
    expect(loss[0]?.data).toEqual([40, 40, 40]);
  });

  it('an ended run keeps its charts and tables, shows no rates and takes its numbers from the run totals', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3), '1': fx.historySamples(0, 3) },
      totals: { '0': fx.totals(), '1': fx.totals({ loss_drops: 140 }) },
    });
    renderApp('/');
    await findTitles(TITLES);
    expect(pillText('pill-tx')).toBe('0 Bit/s (0 Pps)');
    expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument();
    expect(bodyCells(card(0), 'Lost Frames')).toEqual(['100.00', '1.00 %', '9.00']);
    expect(bodyCells(card(0), 'TX L1')).toEqual(['0 Bit/s', '0 Bit/s', '0 Bit/s', '0 Bit/s']);
    expect(chartIn(card(0), 'Rate chart of link 0').data.datasets[0]?.data).toHaveLength(3);
    expect(chartIn(card(1), 'Distribution of the dropped packets by cause').data.datasets[0]?.data?.[0]).toBe(140);
    expect(chartIn(summary(), 'Distribution of the received frames by size').data.datasets[0]?.data).toEqual(
      fx.totals().frame_sizes.map((n) => 2 * n),
    );
  });

  it('a new run clears the page: the old series is not shown while the run starts', async () => {
    const user = userEvent.setup();
    const c = fakeController('stopped');
    serveHistory({
      startedAt: 1762000000,
      samples: { '0': fx.historySamples(0, 3) },
      links: fx.historyLinks().slice(0, 1),
    });
    renderApp('/');
    await findTitles([must(TITLES[0])]);
    await clickWhenEnabled(user, 'Start');
    // the controller still serves the old run until the new one exists: the tabs fall back to the stored links
    await waitFor(() => expect(c.state).toBe('starting'));
    await findTitles(TITLES);
    expect(chartIn(card(0), 'Rate chart of link 0').data.datasets[0]?.data).toHaveLength(0);
  });
});

describe('Dashboard: delay and IAT histograms', () => {
  beforeEach(() => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(twoDirections());
  });

  it('a link with delay statistics offers the histograms, one without it does not', async () => {
    renderApp('/');
    await findTitles(TITLES);
    const names = (panel: HTMLElement) =>
      within(panel)
        .getAllByRole('radio')
        .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent);
    expect(names(card(0))).toEqual(['Traffic rates', 'Packet loss', 'Delay histogram', 'IAT histogram']);
    expect(names(card(1))).toEqual(['Traffic rates', 'Packet loss']);
    expect(radioIn(summary(), 'Delay histogram')).toBeDisabled();
  });

  it('the delay histogram view loads the run histogram: tables, probabilities of the bins and the live mean', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await findTitles(TITLES);
    await user.click(radioIn(card(0), 'Delay histogram'));
    const panel = card(0);
    await waitFor(() => expect(hasChart(panel, 'Delay histogram of link 0')).toBe(true));
    expect(api.getDelayHistogram).toHaveBeenCalledWith(0);

    // probability = count / everything recorded (bins 153 + underflow 3 + overflow 7)
    const bars = chartIn(panel, 'Delay histogram of link 0').data.datasets[0]?.data ?? [];
    [1, 10, 100, 40, 2].forEach((n, i) => {
      expect(must(bars[i])).toBeCloseTo((n / 163) * 100, 9);
    });
    expect(bodyCells(panel, 'Bin Range')).toEqual(['[900.00 µs, 1.40 ms]', '5', '100.00 µs']);
    expect(bodyCells(panel, 'Delay')).toEqual(['1.00 ms', '50.00 µs', '10', '163']);
    expect(bodyCells(panel, 'p50')).toEqual(['1.05 ms', '1.15 ms', '1.25 ms', '1.29 ms']);
  });

  it('the IAT view loads the IAT histogram and takes its mean from the bins', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await findTitles(TITLES);
    await user.click(radioIn(card(0), 'IAT histogram'));
    const panel = card(0);
    await waitFor(() => expect(hasChart(panel, 'IAT histogram of link 0')).toBe(true));
    expect(api.getIatHistogram).toHaveBeenCalledWith(0);
    expect(api.getDelayHistogram).not.toHaveBeenCalled();
    // bins 5, 50, 20, 10, 2 at the centres 1, 3, 5, 7, 9 us: mean (5 + 150 + 100 + 70 + 18) / 87 us
    expect(bodyCells(panel, 'IAT')[0]).toBe('3.94 µs');
    expect(bodyCells(panel, 'IAT')[3]).toBe('91');
  });

  it('a failing histogram request is reported where the chart would be', async () => {
    const user = userEvent.setup();
    api.getDelayHistogram.mockRejectedValue(new Error('no histogram yet'));
    renderApp('/');
    await findTitles(TITLES);
    await user.click(radioIn(card(0), 'Delay histogram'));
    expect(await screen.findByTestId('histogram-error')).toHaveTextContent('no histogram yet');
  });
});

describe('Dashboard: running configuration', () => {
  it('local links equal to the running configuration need no restart', async () => {
    api.getEmulation.mockResolvedValue(fx.runningEmulation());
    renderApp('/');
    await findState('running');
    expect(screen.queryByTestId('pending-restart')).not.toBeInTheDocument();
  });

  it('a changed local link shows "restart needed"', async () => {
    seedStore({ links: [fx.link({ config: fx.config({ buffer_capacity: 7 }) }), fx.linkRev()] });
    api.getEmulation.mockResolvedValue(fx.runningEmulation());
    renderApp('/');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
  });

  it('a browser without links can load the running configuration into its local document', async () => {
    const user = userEvent.setup();
    seedStore({ links: [] });
    api.getEmulation.mockResolvedValue(fx.runningEmulation());
    renderApp('/');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(storedDoc().links.map((l) => l.name)).toEqual(['a-b', 'b-a']));
    expect(screen.queryByRole('button', { name: 'Load running configuration' })).not.toBeInTheDocument();
    expect(screen.queryByTestId('pending-restart')).not.toBeInTheDocument();
  });
});
