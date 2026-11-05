import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState } from '../api/types';
import { pillText } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Spec 46 (user 2026-11-05): the left column of the dashboard's top row looks and reads exactly like P4TG's
// SendReceiveMonitor + Status (Configuration GUI/src/components/{SendReceiveMonitor,Status}.tsx). Spec:
// .git/claude-specs/spec46-p4tg-top-row.md. The four formatters are in common/format.test.ts.
//
// Order of the children of `top-pills` (data-testid):  pill-tx, pill-rx, pill-status (only while running), pill-elapsed
// (elapsed seconds > 0), loss-pct (rx_bps > 0 and 1 - tx_bps / rx_bps > 0.001).
//
// CHOICE (how the P4TG look is pinned): INLINE STYLES. P4TG styles with styled-components, jsdom renders no stylesheet, and the
// spec gives the values per element but no class names. So the tests read `element.style` (React's `style` prop) of the
// pills and of the arrow icons: background `var(--color-...)`, padding and so on, longhands for the padding (jsdom folds the
// shorthand), `background` or `background-color`, and white as "#fff" in any spelling. A stylesheet class would fail these
// tests on purpose. The stylesheet itself has no `.status-pill` rule any more (assets/stylesSpec46.test.ts).
// CHOICE: `loss-pct` is a span wrapping the icon and the text, as the spec allows; it is a direct child of `top-pills`.
// CHOICE: before any run both rate pills read "–" (as before spec 46), after a run ended "0 Bit/s (0 Pps)".

const STATES: EmulationState[] = ['stopped', 'starting', 'running', 'stopping', 'failed'];
const ZERO_COUNTERS = {
  loss_drops: 0,
  limiter_drops: 0,
  buffer_full_drops: 0,
  tx_drops: 0,
  nic_rx_missed: 0,
  nic_rx_nombuf: 0,
  nic_rx_port_buffer_discards: 0,
  nic_rx_phy_discards: 0,
  nic_tx_phy_discards: 0,
  dpds_rejects: 0,
};

/** One direction with the given drops (every other counter 0) and rates. */
const dir = (n: number, over: Partial<ReturnType<typeof fx.dirStats>> = {}, drops: object = {}) =>
  fx.dirStats(n, { counters: fx.counters({ ...ZERO_COUNTERS, ...drops }), ...over });

async function runWith(dirs: ReturnType<typeof fx.dirStats>[], over: Partial<ReturnType<typeof fx.stats>> = {}) {
  api.getEmulation.mockResolvedValue(fx.emulation('running'));
  api.getStats.mockResolvedValue(fx.stats(dirs, over));
  renderApp('/');
  await screen.findByTestId('pill-tx');
  // the TX pill renders before the first /emulation answer; the Status shows once "running" is known (reading
  // pill-status right after pill-tx was flaky under load)
  await screen.findByTestId('pill-status');
}

const ZERO_TOTALS = {
  loss_drops: 0,
  limiter_drops: 0,
  buffer_full_drops: 0,
  nic_tx_drops: 0,
  nic_rx_missed: 0,
  nic_rx_nombuf: 0,
  nic_rx_port_buffer_discards: 0,
  nic_rx_phy_discards: 0,
  nic_tx_phy_discards: 0,
};

/** The run has ended (no directions, frozen clock), the history holds the totals of the run. */
async function endedRun(state: 'stopped' | 'stopping' | 'failed', elapsed_ms: number, drops: object = {}) {
  serveHistory({
    startedAt: 1762000000,
    samples: { '0': fx.historySamples(1, 2) },
    totals: { '0': fx.totals({ ...ZERO_TOTALS, ...drops }) },
  });
  api.getEmulation.mockResolvedValue(fx.emulation(state, { directions: [] }));
  api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms }));
  renderApp('/');
  await screen.findByTestId('pill-tx');
}

const pills = () => screen.getByTestId('top-pills');
const el = (testId: string) => screen.getByTestId(testId);
const prop = (e: HTMLElement, name: string) => e.style.getPropertyValue(name);
const background = (e: HTMLElement) => prop(e, 'background') || prop(e, 'background-color');
const isWhite = (v: string) => ['#fff', '#ffffff', 'white', 'rgb(255, 255, 255)'].includes(v.trim().toLowerCase());

/** The 4 padding sides as [top, right, bottom, left]. */
const padding = (e: HTMLElement) => ['top', 'right', 'bottom', 'left'].map((s) => prop(e, `padding-${s}`));

/** The look the P4TG elements share: white text, 10px radius, inline block. */
function expectBox(e: HTMLElement) {
  expect(isWhite(prop(e, 'color'))).toBe(true);
  expect(prop(e, 'border-radius')).toBe('10px');
  expect(prop(e, 'display')).toBe('inline-block');
}

describe('Dashboard (spec 46): the top row column', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  it('is `col-8 text-start` without flex and gap utilities (P4TG uses inline-blocks with margins)', async () => {
    await runWith([dir(0), dir(1)]);
    expect(pills()).toHaveClass('col-8', 'text-start');
    const classes = Array.from(pills().classList);
    expect(classes.filter((c) => c === 'd-flex' || c.startsWith('gap-') || c.startsWith('flex-'))).toEqual([]);
  });

  it('the order: TX, RX, Status, duration, loss percentage (everything shows)', async () => {
    await runWith(
      [dir(0, { rx_bps: 6.1e9, tx_bps: 5.05e9 }, { loss_drops: 5 }), dir(1, { rx_bps: 3.15e9, tx_bps: 2.2e9 })],
      { elapsed_ms: 192_000 },
    );
    const ids = ['pill-tx', 'pill-rx', 'pill-status', 'pill-elapsed', 'loss-pct'];
    const found = ids.map((id) => within(pills()).getByTestId(id));
    for (const [i, e] of found.entries()) expect(e.parentElement, ids[i]).toBe(pills());
    for (let i = 1; i < found.length; i += 1) {
      expect(found[i - 1].compareDocumentPosition(found[i]) & Node.DOCUMENT_POSITION_FOLLOWING, ids[i]).toBeTruthy();
    }
    // nothing else between them: the direct children are exactly these five
    expect(Array.from(pills().children)).toEqual(found);
  });

  it('there is no pill-lost and no pill-dropped any more, even with drops', async () => {
    await runWith([dir(0, {}, { loss_drops: 5 })]);
    expect(screen.queryByTestId('pill-lost')).not.toBeInTheDocument();
    expect(screen.queryByTestId('pill-dropped')).not.toBeInTheDocument();
  });
});

describe('Dashboard (spec 46): the TX and RX pills', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  const LIVE = [
    dir(0, { rx_bps: 6.1e9, tx_bps: 5.05e9, rx_pps: 4.6e6, tx_pps: 3.9e6 }),
    dir(1, { rx_bps: 3.15e9, tx_bps: 2.2e9, rx_pps: 2.71e6, tx_pps: 2.41e6 }),
  ];

  // User 2026-11-05: P4TG's pills show the live L1 rates (SendReceiveMonitor: tx_rate_l1 / rx_rate_l1), so DPDS shows
  // L1 = L2 + 20 B x 8 x pps too: RX 9.25e9 + 7.31e6 x 160 = 10.4196e9, TX 7.25e9 + 6.31e6 x 160 = 8.2596e9.
  it('read "<formatBits> (<formatPacketRate>)" of the summed L1 rates, with no "RX"/"TX" label', async () => {
    await runWith(LIVE);
    await waitFor(() => expect(pillText('pill-rx')).toBe('10.42 Gbit/s (7.31 Mpps)'));
    expect(pillText('pill-tx')).toBe('8.26 Gbit/s (6.31 Mpps)');
    expect(el('pill-rx').textContent).not.toMatch(/RX|TX/);
    expect(el('pill-tx').textContent).not.toMatch(/RX|TX/);
  });

  it('use P4TG units: Kbit/s and Kpps with a capital K, no trailing zeros, no unit padding', async () => {
    await runWith([dir(0, { rx_bps: 1000, rx_pps: 1000, tx_bps: 1500, tx_pps: 1500 })]);
    // L1: 1000 + 1000 x 160 = 161000 and 1500 + 1500 x 160 = 241500
    await waitFor(() => expect(pillText('pill-rx')).toBe('161 Kbit/s (1 Kpps)'));
    expect(pillText('pill-tx')).toBe('241.5 Kbit/s (1.5 Kpps)');
  });

  it('a stopped emulation shows 0 Bit/s (0 Pps) after a run, "–" before any run', async () => {
    await endedRun('stopped', 5000);
    expect(pillText('pill-rx')).toBe('0 Bit/s (0 Pps)');
    expect(pillText('pill-tx')).toBe('0 Bit/s (0 Pps)');
  });

  it('read "–" before any run', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    renderApp('/');
    await screen.findByTestId('pill-tx');
    expect(pillText('pill-rx')).toBe('–');
    expect(pillText('pill-tx')).toBe('–');
  });

  describe('the arrow icons', () => {
    it.each(STATES)('"%s": TX has the green up arrow, RX the yellow down arrow, first in the pill', async (state) => {
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      api.getStats.mockResolvedValue(
        state === 'running' ? fx.stats([dir(0), dir(1)]) : fx.stats([], { stale: true, age_ms: undefined }),
      );
      renderApp('/');
      await screen.findByTestId('pill-tx');
      const tx = must(el('pill-tx').querySelector('i'), 'TX icon');
      const rx = must(el('pill-rx').querySelector('i'), 'RX icon');
      expect(el('pill-tx').firstElementChild).toBe(tx);
      expect(el('pill-rx').firstElementChild).toBe(rx);
      expect(tx).toHaveClass('bi', 'bi-arrow-up-circle-fill');
      expect(rx).toHaveClass('bi', 'bi-arrow-down-circle-fill');
      expect(prop(tx, 'color')).toBe('var(--color-okay)');
      expect(prop(rx, 'color')).toBe('var(--color-yellow)');
      expect(prop(tx, 'padding-right')).toBe('5px');
      expect(prop(rx, 'padding-right')).toBe('5px');
    });
  });

  it.each(['pill-tx', 'pill-rx'])("%s is styled like P4TG's Stat", async (id) => {
    await runWith([dir(0), dir(1)]);
    const e = el(id);
    expect(background(e)).toBe('var(--color-secondary)');
    expect(padding(e)).toEqual(['10px', '15px', '10px', '15px']);
    expectBox(e);
    expect(prop(e, 'min-width')).toBe('250px');
    expect(prop(e, 'text-align')).toBe('center');
    expect(prop(e, 'margin-right')).toBe('10px');
    expect(prop(e, 'margin-bottom')).toBe('10px');
  });
});

describe('Dashboard (spec 46): Status', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  it.each(STATES.filter((s) => s !== 'running'))('"%s": there is no Status', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    renderApp('/');
    await screen.findByTestId('pill-tx');
    expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument();
  });

  it('"stopping" (the drain) has no Status although the run dropped frames', async () => {
    await endedRun('stopping', 5000, { loss_drops: 50 });
    expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument();
  });

  it('a stopped run that dropped frames has no Status', async () => {
    await endedRun('stopped', 5000, { loss_drops: 50 });
    expect(screen.queryByTestId('pill-status')).not.toBeInTheDocument();
  });

  it('while running without drops it reads "Status: Ok", green, with no tooltip', async () => {
    await runWith([dir(0), dir(1)]);
    const s = el('pill-status');
    expect(pillText('pill-status')).toBe('Status: Ok');
    expect(background(s)).toBe('var(--color-okay)');
    await userEvent.setup().hover(s);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });

  it('CHOICE: while running before the first stats reply it is "Status: Ok" (lost is 0)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockImplementation(() => new Promise(() => undefined));
    renderApp('/');
    await screen.findByTestId('pill-status');
    expect(pillText('pill-status')).toBe('Status: Ok');
  });

  it.each([
    [5, 'Status: Error. Lost 5.00 frames.'],
    [20, 'Status: Error. Lost 20.00 frames.'],
    [7770, 'Status: Error. Lost 7.77 K frames.'],
    [1_234_567, 'Status: Error. Lost 1.23 M frames.'],
    [2_500_000_000, 'Status: Error. Lost 2.50 B frames.'],
  ])('with %s lost frames it reads "%s", in the P4TG red', async (lost, text) => {
    await runWith([dir(0, {}, { loss_drops: lost })]);
    await waitFor(() => expect(pillText('pill-status')).toBe(text));
    expect(background(el('pill-status'))).toBe('var(--color-primary)');
  });

  it('counts all nine causes, over all links (the Summary total)', async () => {
    await runWith([
      dir(0, {}, { loss_drops: 4000, limiter_drops: 1000, buffer_full_drops: 500 }),
      dir(1, {}, { tx_drops: 1000, nic_rx_missed: 700, nic_rx_nombuf: 570, dpds_rejects: 99_999 }),
    ]);
    await waitFor(() => expect(pillText('pill-status')).toBe('Status: Error. Lost 7.77 K frames.'));
  });

  it('spec 47 F2: the three new NIC causes are in the lost total as well (all nine, both links)', async () => {
    await runWith([
      dir(0, {}, { loss_drops: 4000, limiter_drops: 1000, buffer_full_drops: 500, nic_rx_port_buffer_discards: 100 }),
      dir(
        1,
        {},
        { tx_drops: 1000, nic_rx_missed: 600, nic_rx_nombuf: 470, nic_rx_phy_discards: 70, nic_tx_phy_discards: 260 },
      ),
    ]);
    await waitFor(() => expect(pillText('pill-status')).toBe('Status: Error. Lost 8.00 K frames.'));
  });

  it('spec 47 C4: the NIC diagnostics (crc, symbol, length errors) are in no loss total', async () => {
    await runWith([dir(0, {}, { nic_rx_crc_errors: 9, nic_rx_symbol_errors: 9, nic_rx_length_errors: 9 })]);
    expect(pillText('pill-status')).toBe('Status: Ok');
  });

  it.each([
    'loss_drops',
    'limiter_drops',
    'buffer_full_drops',
    'tx_drops',
    'nic_rx_missed',
    'nic_rx_nombuf',
    'nic_rx_port_buffer_discards',
    'nic_rx_phy_discards',
    'nic_tx_phy_discards',
  ])('%s alone turns it to Error', async (key) => {
    await runWith([dir(0, {}, { [key]: 3 })]);
    await waitFor(() => expect(pillText('pill-status')).toBe('Status: Error. Lost 3.00 frames.'));
  });

  it('dpds_rejects alone are not a loss', async () => {
    await runWith([dir(0, {}, { dpds_rejects: 90 })]);
    expect(pillText('pill-status')).toBe('Status: Ok');
  });

  it('the tooltip on the error text shows the exact count, without grouping or units', async () => {
    await runWith([dir(0, {}, { loss_drops: 7770 })]);
    const user = userEvent.setup();
    const text = await within(el('pill-status')).findByText(/Lost 7\.77 K frames/);
    await user.hover(text);
    const tip = await screen.findByRole('tooltip');
    expect(tip.textContent?.trim()).toBe('7770');
    await user.unhover(text);
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('the tooltip count is the exact total of a million-scale loss, "1234567"', async () => {
    await runWith([dir(0, {}, { loss_drops: 1_000_000, nic_rx_missed: 234_567 })]);
    const user = userEvent.setup();
    await user.hover(await within(el('pill-status')).findByText(/Lost 1\.23 M frames/));
    expect((await screen.findByRole('tooltip')).textContent?.trim()).toBe('1234567');
  });

  it('the tooltip sits on top of the text', async () => {
    await runWith([dir(0, {}, { loss_drops: 7770 })]);
    const user = userEvent.setup();
    await user.hover(await within(el('pill-status')).findByText(/Lost 7\.77 K frames/));
    const tip = await screen.findByRole('tooltip');
    // react-bootstrap's Tooltip: the placement is a class (bs-tooltip-*) or a popper attribute
    expect(`${tip.className} ${tip.getAttribute('data-popper-placement') ?? ''}`).toMatch(/top/);
  });

  it('turns red when drops appear and green again when they are gone (colour and text follow the poll)', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([dir(0)]));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(background(el('pill-status'))).toBe('var(--color-okay)');
    api.getStats.mockResolvedValue(fx.stats([dir(0, {}, { loss_drops: 5 })]));
    await tick(2500);
    expect(pillText('pill-status')).toBe('Status: Error. Lost 5.00 frames.');
    expect(background(el('pill-status'))).toBe('var(--color-primary)');
    api.getStats.mockResolvedValue(fx.stats([dir(0)]));
    await tick(2500);
    expect(pillText('pill-status')).toBe('Status: Ok');
    expect(background(el('pill-status'))).toBe('var(--color-okay)');
  });

  it("is styled like P4TG's StatusIndicator", async () => {
    await runWith([dir(0, {}, { loss_drops: 5 })]);
    const e = el('pill-status');
    expect(padding(e)).toEqual(['10px', '15px', '10px', '15px']);
    expectBox(e);
    expect(prop(e, 'margin-right')).toBe('10px');
  });
});

describe('Dashboard (spec 46): the duration', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  it.each([
    [1000, '0h 0m 1s'],
    [65_400, '0h 1m 5s'], // floor of the seconds
    [192_000, '0h 3m 12s'],
    [3_725_999, '1h 2m 5s'],
    [90_000_000, '25h 0m 0s'],
  ])('elapsed_ms %s reads "%s"', async (ms, text) => {
    await runWith([dir(0), dir(1)], { elapsed_ms: ms });
    expect(pillText('pill-elapsed')).toBe(text);
  });

  it('is absent without elapsed_ms', async () => {
    await runWith([dir(0), dir(1)]);
    expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
  });

  it('is absent at elapsed_ms 0', async () => {
    await runWith([dir(0), dir(1)], { elapsed_ms: 0 });
    expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
  });

  // Spec 46 item 4 (amended): like P4TG, a stopped or failed DPDS shows no duration. Was: 'keeps the frozen time after the run ended'.
  it.each(['stopped', 'failed'] as const)(
    'is hidden once the emulation is "%s", although the stats carry the frozen time',
    async (state) => {
      await endedRun(state, 305_400);
      expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
    },
  );

  it.each(['starting', 'running', 'stopping'] as const)('still shows while the emulation is "%s"', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    api.getStats.mockResolvedValue(fx.stats([dir(0)], { elapsed_ms: 305_400 }));
    renderApp('/');
    await screen.findByTestId('pill-tx');
    await waitFor(() => expect(pillText('pill-elapsed')).toBe('0h 5m 5s'));
  });

  it('is hidden when the emulation goes from running to stopped, and shows again on the next run', async () => {
    vi.useFakeTimers();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([dir(0)], { elapsed_ms: 4000 }));
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(pillText('pill-elapsed')).toBe('0h 0m 4s');
    api.getEmulation.mockResolvedValue(fx.emulation('stopped', { directions: [] }));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined, elapsed_ms: 9000 }));
    await tick(2500);
    expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([dir(0)], { elapsed_ms: 2000 }));
    await tick(2500);
    expect(pillText('pill-elapsed')).toBe('0h 0m 2s');
  });

  it('is absent before a run (the stats carry no elapsed_ms)', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    renderApp('/');
    await screen.findByTestId('pill-tx');
    expect(screen.queryByTestId('pill-elapsed')).not.toBeInTheDocument();
  });

  it("is styled like P4TG's Time", async () => {
    await runWith([dir(0), dir(1)], { elapsed_ms: 192_000 });
    const e = el('pill-elapsed');
    expect(background(e)).toBe('var(--color-secondary)');
    expect(padding(e)).toEqual(['10px', '5px', '10px', '5px']);
    expectBox(e);
    expect(prop(e, 'margin-right')).toBe('10px');
    expect(prop(e, 'margin-bottom')).toBe('10px');
    expect(prop(e, 'min-width')).toBe('140px');
    expect(prop(e, 'max-width')).toBe('140px');
    expect(prop(e, 'text-align')).toBe('center');
  });
});

describe('Dashboard (spec 46): the loss percentage, 100 * (1 - TX / RX)', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
  });

  const rates = (rx: number, tx: number) => [dir(0, { rx_bps: rx, tx_bps: tx })];

  it.each([
    [1e9, 0.5e9, '50.00 %'],
    [1e9, 0.998e9, '0.20 %'],
    [9.25e9, 7.25e9, '21.62 %'],
  ])('rx %s, tx %s: reads "%s" with the red exclamation icon in front', async (rx, tx, text) => {
    await runWith(rates(rx, tx));
    const e = el('loss-pct');
    expect(e.textContent?.replace(/\s+/g, ' ').trim()).toBe(text);
    const icon = must(e.querySelector('i'), 'the icon');
    expect(icon).toHaveClass('bi', 'bi-exclamation-circle-fill', 'text-danger');
    expect(e.firstElementChild).toBe(icon);
  });

  it('is the ratio of the sums over the links (default fixtures: 19 vs 16.5 Gbit/s)', async () => {
    await runWith([dir(0), dir(1)]);
    expect(pillText('loss-pct')).toBe('13.16 %');
  });

  it.each([
    ['equal rates', 1e9, 1e9],
    ['a loss of 0.05 %, below the 0.1 % threshold', 1e9, 0.9995e9],
    ['TX above RX', 1e9, 1.2e9],
    ['no RX at all', 0, 0],
    ['TX without RX', 0, 1e9],
  ])('is absent for %s', async (_name, rx, tx) => {
    await runWith(rates(rx, tx));
    expect(screen.queryByTestId('loss-pct')).not.toBeInTheDocument();
  });

  it('is absent before a run and after it ended (the rates are 0 then)', async () => {
    await endedRun('stopped', 5000, { loss_drops: 99 });
    expect(screen.queryByTestId('loss-pct')).not.toBeInTheDocument();
  });

  it('does not depend on the drop counters', async () => {
    await runWith([dir(0, { rx_bps: 1e9, tx_bps: 0.5e9 })]);
    expect(pillText('pill-status')).toBe('Status: Ok');
    expect(pillText('loss-pct')).toBe('50.00 %');
  });
});
