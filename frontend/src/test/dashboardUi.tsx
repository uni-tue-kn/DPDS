// Helpers and the DOM contract of the dashboard tests.
//
// Layout, top to bottom:
//   top row    data-testid="top-row": `top-pills` (pill-tx, pill-rx, pill-status, pill-elapsed, loss-pct, in DOM order; see
//              dashboardTopRow.test.tsx) and `run-buttons` (see RUN_BUTTONS). The pills are live statistics: after a run TX/RX read
//              "0 Bit/s (0 Pps)" and pill-elapsed keeps the frozen elapsed_ms; before any run TX/RX read "–" and there is no elapsed
//              pill. pill-status exists only while running: "Status: Ok" or "Status: Error. Lost <N> frames.", N = the sum of all
//              causes of the Summary's drop counts. There is no state badge; `expectState()` tells the state from the run buttons.
//   notices    data-testid="notices", a row after the top row: "Restart needed" (pending-restart), stale-indicator, stats-warning,
//              history-warning, and every role="alert" (action error, poll error, emulation error). Never inside the top row.
//   chart area a react-bootstrap Tabs: role "tablist", the first role "tab" is "Summary" (selected by default), then one tab per
//              link in index order, titled "name: ingress → egress" (netdev labels), without a badge. Before any run (no history
//              samples, state not running/starting) the tabs are the Summary plus one per STORED link; a running or ended run takes
//              them from the run. Each tabpanel holds the chart (role "img" with an aria-label), the radiogroup "Chart" (CHART_RADIOS),
//              the histogram tables in the histogram views, the rate and loss tables, and the donut row.
//   tables     a link's "Delay histogram" view has one `.row` of three `.col-md-4` columns above the bar chart, each a
//              `table.table-striped.table-bordered.table-hover.table-sm` with a `thead.table-dark` and exactly one body row:
//              "Histogram config", "Values" and "Percentiles"; their header tooltips show on hover. The rate table (TX L1 | RX L1 |
//              TX L2 | RX L2) and the loss table (Lost Frames | Frame Loss Ratio | Out of Order) share ONE `.row.mb-3` of two
//              `.col-12.col-md-6` (they stack below md).
//   donut row  ONE `.row` of two `.col-12.col-md-6`: the loss donut (left, class `donut-divider`) and the frame size donut (right),
//              each chart in an element with role "img" found by its aria-label (see the label helpers below). Several `chart-doughnut`
//              / `chart-bar` stubs share a panel: tests pick one through lossDonut(), frameDonut(), delayBar() or iatBar(), never by
//              test id alone. The loss donut's React legend `donut-legend` has one listitem per cause with a `legend-swatch` and a
//              button "About <label>" whose tooltip is the explanation; with all counts 0 the chart is one grey placeholder ring
//              and "No drops" is shown.
//   radios     "Traffic rates", "Packet loss", "Delay histogram", "IAT histogram". The two histogram radios are enabled where delay
//              statistics are on (also after the run ended), disabled in the Summary and before any run, absent for a link without
//              delay statistics. The IAT view polls getIatHistogram(dir) only while it is chosen and the tab is shown.
//   log        only for state "failed" and a non-empty log_tail: a closed <details> "Show log" that holds data-testid="log-tail".
//
// `tabs()`, `tab(d)`, `hasTab(d)`, `card(d)` and the other helpers below address the LINK tabs only; `summaryTab()`, `summary()`
// and `summaryRadio()` address the Summary tab. `card(d)` selects the tab of link position d (a no-op when it is selected) and
// returns that tab's tabpanel (found by aria-labelledby; other panels may stay mounted), so `within(card(d))` queries that link's
// chart area.
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import type { EmulationState } from '../api/types';
import { must } from './must';

/** The title of the Summary tab. */
export const SUMMARY = 'Summary';

const isSummary = (t: HTMLElement) => (t.textContent ?? '').trim() === SUMMARY;

/** Every tab of the page, in order, the Summary tab included. */
export const allTabs = () => screen.queryAllByRole('tab');

/** The link tabs of the page, in order (the Summary tab is not one of them). */
export const tabs = () => allTabs().filter((t) => !isSummary(t));

/** The Summary tab. */
export const summaryTab = (): HTMLElement => must(allTabs().find(isSummary), 'the Summary tab');

/** True iff the page has a Summary tab. */
export const hasSummary = () => allTabs().some(isSummary);

/** The tab of link position `d`. */
export const tab = (d: number): HTMLElement => must(tabs()[d], `tab ${d}`);

/** True iff there is a tab at position `d`. */
export const hasTab = (d: number) => tabs().length > d;

/**
 * The tabpanel of `t`: the one whose aria-labelledby names the tab (react-bootstrap sets it). An implementation may keep the
 * panels of the other tabs mounted, so "the tabpanel" is never found by role alone.
 */
export function panelOf(t: HTMLElement): HTMLElement {
  const panel = screen
    .getAllByRole('tabpanel', { hidden: true })
    .find((p) => p.getAttribute('aria-labelledby') === t.id);
  return must(panel, `the tabpanel labelled by tab ${t.id}`);
}

/** The tabpanel of the selected tab: the chart area that shows. */
export function activePanel(): HTMLElement {
  const selected = allTabs().filter((t) => t.getAttribute('aria-selected') === 'true');
  if (selected.length !== 1) throw new Error(`expected exactly one selected tab, found ${selected.length}`);
  return panelOf(selected[0]);
}

/** Selects tab `d` and returns its tabpanel (the one chart area that shows). */
export function card(d: number): HTMLElement {
  const t = tab(d);
  if (t.getAttribute('aria-selected') !== 'true') fireEvent.click(t);
  return panelOf(t);
}

/** Selects the Summary tab and returns its tabpanel (the one chart area that shows). */
export function summary(): HTMLElement {
  const t = summaryTab();
  if (t.getAttribute('aria-selected') !== 'true') fireEvent.click(t);
  return panelOf(t);
}

/** The radio `name` of the Summary tab's chart area. */
export const summaryRadio = (name: string) => within(summary()).getByRole('radio', { name });

/** Waits until there are `n` link tabs; selects nothing. */
export async function findTabs(n: number): Promise<void> {
  await waitFor(() => expect(tabs().length).toBeGreaterThanOrEqual(n));
}

/** Waits until there is a tab `d`, then returns `card(d)` (which selects that tab). */
export async function findCard(d: number): Promise<HTMLElement> {
  await waitFor(() => expect(tabs().length).toBeGreaterThan(d));
  return card(d);
}

/** The labels of the role "img" boxes around the donuts and the bar charts. */
export const LOSS_DONUT_LABEL = 'Distribution of the dropped packets by cause';
export const FRAME_DONUT_LABEL = 'Distribution of the received frames by size';
export const DELAY_BAR_LABEL = /^Delay histogram of link \d+$/;
export const IAT_BAR_LABEL = /^IAT histogram of link \d+$/;

/** The chart stub inside the role "img" box `name` of `scope`; throws when there is no such box or no chart in it. */
const stubIn = (scope: HTMLElement, name: string | RegExp, testId: string) =>
  within(within(scope).getByRole('img', { name })).getByTestId(testId);
const queryStubIn = (scope: HTMLElement, name: string | RegExp, testId: string) =>
  within(scope).queryByRole('img', { name }) === null ? null : stubIn(scope, name, testId);

/** The loss donut's Doughnut stub in `scope` (a tabpanel). */
export const lossDonut = (scope: HTMLElement) => stubIn(scope, LOSS_DONUT_LABEL, 'chart-doughnut');
export const queryLossDonut = (scope: HTMLElement) => queryStubIn(scope, LOSS_DONUT_LABEL, 'chart-doughnut');
/** The frame size donut's Doughnut stub in `scope`. */
export const frameDonut = (scope: HTMLElement) => stubIn(scope, FRAME_DONUT_LABEL, 'chart-doughnut');
export const queryFrameDonut = (scope: HTMLElement) => queryStubIn(scope, FRAME_DONUT_LABEL, 'chart-doughnut');
/** The delay histogram's Bar stub in `scope` (present only in the "Delay histogram" view). */
export const delayBar = (scope: HTMLElement) => stubIn(scope, DELAY_BAR_LABEL, 'chart-bar');
export const queryDelayBar = (scope: HTMLElement) => queryStubIn(scope, DELAY_BAR_LABEL, 'chart-bar');
export const findDelayBar = (scope: HTMLElement) => waitFor(() => delayBar(scope));
/** The radios of a chart area that has a histogram, in order. A link without delay statistics has the first and the last only. */
export const CHART_RADIOS = ['Traffic rates', 'Packet loss', 'Delay histogram', 'IAT histogram'];
/** The headers of the IAT view's three tables (the delay ones with "IAT" in place of "Delay"). */
export const IAT_TABLE_HEADERS = [
  ['Bin Range', '#Bins', 'Bin Width'],
  ['IAT', 'σ(IAT)', 'Outlier', 'Total #Packets'],
  ['p50', 'p90', 'p99', 'p99.9'],
];
/** The IAT histogram's Bar stub in `scope`. */
export const iatBar = (scope: HTMLElement) => stubIn(scope, IAT_BAR_LABEL, 'chart-bar');
export const queryIatBar = (scope: HTMLElement) => queryStubIn(scope, IAT_BAR_LABEL, 'chart-bar');
export const findIatBar = (scope: HTMLElement) => waitFor(() => iatBar(scope));
/** Chooses the "IAT histogram" radio of link `d`, waits for its bar chart and returns the link's tabpanel. */
export async function showIat(d: number): Promise<HTMLElement> {
  fireEvent.click(radio(d, 'IAT histogram'));
  await findIatBar(card(d));
  return card(d);
}

const headersOf = (t: HTMLElement) =>
  within(t)
    .queryAllByRole('columnheader')
    .map((h) => (h.textContent ?? '').trim());

/** The headers of the two tables that are not histogram tables. */
export const RATE_HEADERS = ['TX L1', 'RX L1', 'TX L2', 'RX L2'];
export const LOSS_HEADERS = ['Lost Frames', 'Frame Loss Ratio', 'Out of Order'];

const tablesWith = (scope: HTMLElement, headers: string[]) =>
  within(scope)
    .queryAllByRole('table')
    .filter((t) => headersOf(t).join('|') === headers.join('|'));

/** The rate table (TX L1, RX L1, TX L2, RX L2) of `scope`; exactly one. */
export const rateTable = (scope: HTMLElement): HTMLElement => {
  const found = tablesWith(scope, RATE_HEADERS);
  if (found.length !== 1) throw new Error(`expected one rate table, found ${found.length}`);
  return found[0];
};
/** The loss table (Lost Frames, Frame Loss Ratio, Out of Order) of `scope`; exactly one. */
export const lossTable = (scope: HTMLElement): HTMLElement => {
  const found = tablesWith(scope, LOSS_HEADERS);
  if (found.length !== 1) throw new Error(`expected one loss table, found ${found.length}`);
  return found[0];
};
/** The cells of the one body row of `t`, trimmed text. */
export const bodyCells = (t: HTMLElement): string[] =>
  Array.from(must(within(t).getAllByRole('row')[1], 'the body row').children).map((c) =>
    (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );

/** The histogram tables in `scope`: every table that is neither the rate table nor the loss table. */
export const histTables = (scope: HTMLElement): HTMLElement[] =>
  within(scope)
    .queryAllByRole('table')
    .filter((t) => {
      const h = headersOf(t).join('|');
      return h !== RATE_HEADERS.join('|') && h !== LOSS_HEADERS.join('|');
    });
/** Waits until `scope` has histogram tables, then returns them. */
export const findHistTables = (scope: HTMLElement) =>
  waitFor(() => {
    const t = histTables(scope);
    expect(t.length).toBeGreaterThan(0);
    return t;
  });

/** The radio `name` of link `d`'s chart area. */
export const radio = (d: number, name: string) => within(card(d)).getByRole('radio', { name });

/** The radio `name` of link `d`'s chart area, or null. */
export const queryRadio = (d: number, name: string) => within(card(d)).queryByRole('radio', { name });

/** The names of the radios of link `d`, in DOM order. */
export const radioNames = (d: number) =>
  within(card(d))
    .getAllByRole('radio')
    .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim() ?? '');

/**
 * The label of tab `d` without its badge: the text the tab holds outside any child element with the `badge` class, trimmed.
 * (So "a-b: enp101s0f0 → enp101s0f1" for a live tab and for an ended one.)
 */
export function tabTitle(d: number): string {
  const clone = tab(d).cloneNode(true) as HTMLElement;
  for (const b of Array.from(clone.querySelectorAll('.badge'))) b.remove();
  return (clone.textContent ?? '').replace(/\s+/g, ' ').trim();
}

/** The text of the badge in tab `d` ('' when it has none). */
export function tabBadge(d: number): string {
  return Array.from(tab(d).querySelectorAll('.badge'))
    .map((b) => b.textContent?.trim() ?? '')
    .join(' ');
}

/** The buttons of the run button group, in DOM order. */
export const runButtons = () => within(screen.getByTestId('run-buttons')).queryAllByRole('button');

/** Their accessible names, in DOM order. */
export const runButtonNames = () =>
  runButtons().map((b) => (b.getAttribute('aria-label') ?? b.textContent ?? '').replace(/\s+/g, ' ').trim());

/** One button of the group by its exact name, or null. */
export const runButton = (name: string) =>
  runButtons().find((b) => b.textContent?.replace(/\s+/g, ' ').trim() === name) ?? null;

/** The visible text of a pill, whitespace normalised ("9.25 Gbit/s (7.31 Mpps)"). */
export const pillText = (testId: string) => (screen.getByTestId(testId).textContent ?? '').replace(/\s+/g, ' ').trim();

/**
 * Cause labels of the donut and of the Packet loss chart's datasets, in the order of the donut's segments (nine causes). They carry
 * no counter names; those are in the legend's hover help (CAUSE_HELP).
 */
export const CAUSES = [
  'Loss model',
  'Rate limiter',
  'Buffer full',
  'NIC TX ring full',
  'NIC RX missed',
  'RX buffers exhausted',
  'NIC RX port buffer full',
  'NIC RX port discards',
  'NIC TX port discards',
] as const;

/**
 * The legend tooltips, exactly. The counter names (comma and space separated when there are two) go in brackets right before the
 * final full stop; the first three causes have no counter.
 */
export const CAUSE_HELP: Record<(typeof CAUSES)[number], string> = {
  'Loss model': 'Packets dropped on purpose by the configured loss model (random or Gilbert-Elliott).',
  'Rate limiter': "Packets the limiter refused: the Spacer's backlog or the token bucket's budget was exceeded.",
  'Buffer full': 'Packets that arrived while the delay buffer already held its maximum number of packets.',
  'NIC TX ring full': 'Packets the NIC would not accept for sending because its transmit ring was full (tx_drops).',
  'NIC RX missed': 'Packets the NIC dropped because its receive queues were full (rx_missed_errors).',
  'RX buffers exhausted':
    'Packets lost because no packet buffer (mbuf) was free to receive them (rx_mbuf_allocation_errors).',
  'NIC RX port buffer full':
    "Packets the NIC's port buffer discarded because it was full or congested, summed over all priorities (rx_prio*_buf_discard_packets, rx_prio*_cong_discard_packets).",
  'NIC RX port discards':
    'Packets the NIC port discarded on receive because it had no buffers left (rx_phy_discard_packets).',
  'NIC TX port discards':
    'Packets the NIC accepted for sending but dropped before they reached the wire (tx_phy_discard_packets).',
};

/**
 * The run buttons per state: Stop then Restart while starting/running, the same two (disabled, Stop reading "Draining…") while
 * stopping, only Start otherwise.
 */
export const RUN_BUTTONS: Record<EmulationState, string[]> = {
  stopped: ['Start'],
  starting: ['Stop', 'Restart'],
  running: ['Stop', 'Restart'],
  stopping: ['Draining…', 'Restart'],
  failed: ['Start'],
};

/** The run buttons read like `state` (so the page has taken that state over; stopped/failed and starting/running look alike). */
export const expectState = (state: EmulationState) => expect(runButtonNames(), state).toEqual(RUN_BUTTONS[state]);

/** Waits until the run buttons read like `state`. */
export const findState = (state: EmulationState) => waitFor(() => expectState(state));
