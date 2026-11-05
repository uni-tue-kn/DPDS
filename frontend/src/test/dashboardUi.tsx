// Spec 35 (user 2026-11-05): the minimal, P4TG-like dashboard with a loss donut. Contract of the frontend side, as the
// tests pin it (spec: scratchpad/spec35-dashboard.md, "Frontend layout" F1 to F7). The tester's choices where the spec is silent
// are marked CHOICE here and in the tests.
//
// ## Layout, top to bottom
//   top row    data-testid="top-row", two columns like P4TG's Home.tsx:
//                data-testid="top-pills"   `col-8 text-start`: pill-tx, pill-rx, pill-status, pill-elapsed, loss-pct (DOM order, spec 46:
//                                          P4TG's SendReceiveMonitor, see pages/dashboardTopRow.test.tsx). Spec 43 F3: there is
//                                          no state badge (`state-badge`) any more; `expectState()` below tells the state from the run buttons.
//                                          Spec 37 (user 2026-11-05), like P4TG: the pills are live statistics. While stopped, stopping
//                                          or failed after a run, TX/RX read "0 Bit/s (0 Pps)" and pill-elapsed keeps
//                                          the frozen elapsed_ms. Before any run: "–" for TX/RX, no elapsed pill. Running: the live
//                                          sums. The charts (all tabs, Summary too) and the donuts keep the ended run's history and
//                                          totals; the pills never read those.
//                                          Spec 46 (was spec 43 F2's pill-lost "Lost <N> frames"): pill-status exists only while running,
//                                          "Status: Ok" or "Status: Error. Lost <formatFrameCount(N)> frames.", N = the sum of all six causes of
//                                          the Summary's drop counts; loss-pct is the red icon and "<x.xx> %" when TX falls short of RX.
//                data-testid="run-buttons" the `col-4 text-end` column (spec 43 F8, like P4TG: no ButtonGroup, plain Buttons with
//                                          `mb-1`, no spinners): [Stop][Restart] while starting/running, [Draining…][Restart] (both
//                                          disabled) while stopping, only [Start] otherwise (see RUN_BUTTONS)
//   notices    data-testid="notices", a row after the top row: "Restart needed" (pending-restart), stale-indicator, stats-warning,
//              history-warning, and every role="alert" (action error, poll error, emulation error). Never inside the top row.
//   banner     LoadRunning (unchanged), anywhere below the top row
//   chart area a react-bootstrap Tabs: role "tablist", one role "tab" per link (index order), titled "name: ingress → egress"
//              (netdev labels), with the badge "Stopping" / "Stopped" / "Failed" inside the tab while the run has ended.
//              Inside the active role "tabpanel": the chart (role "img" with an aria-label), centred below it the radiogroup
//              "Chart" with the radios "Traffic rates", "Delay histogram" (only with delay_stats; spec 50 A3: also after the run ended) and "Packet loss" (spec 43 F5), in this order,
//              then the loss donut.
//   donut      chart-doughnut (the Doughnut stub of test/setup.ts), the React legend data-testid="donut-legend" (one listitem per
//              cause, each with a `legend-swatch` and a button "About <label>" whose tooltip is the explanation), or the text
//              With all six counts 0 the chart is still there as one grey placeholder ring (one dataset, one value), the legend is
//              unchanged and the text "No drops" is shown too.
//   log        only for state "failed" and a non-empty log_tail: a closed <details> whose <summary> reads "Show log" and that
//              holds data-testid="log-tail".
//   ports      no ports table on the dashboard, and (spec 43 F6) none on /interfaces either: PortsTable is deleted.
//
// ## Spec 36 (summary tab and histogram tables; spec: .git/claude-specs/spec36-summary-histtables.md)
//   tabs       the FIRST role "tab" is titled exactly "Summary" (no badge) and is selected by default; the link tabs follow in index
//              order, with the same titles and badges as before. No Summary tab (and no tablist) while there is no link tab.
//   summary    inside the Summary tabpanel: the chart (role "img", aria-label starting "Rate chart"/"Loss chart" per view (spec 43 F5: the label may follow the radio name; tests accept both); CHOICE:
//              "of all links"), the radiogroup "Chart" with the radios "Traffic rates", "Delay histogram" (always present there, always
//              disabled, wrapped in a tooltip) and "Packet loss", then the loss donut with the same legend as a link's.
//   tables     inside a link's tabpanel, in the "Delay histogram" view, above the bar chart, in one `.row` of three `.col-md-4`
//              (and `.col-12`) columns; each column holds one `table.table-striped.table-bordered.table-hover.table-sm` with a
//              `thead.table-dark`, a role "columnheader" per column and exactly one body row. The three tables, in DOM order:
//              "Histogram config" (Bin Range, #Bins, Bin Width), "Values" (Dir, Delay, σ(Delay), Outlier, Total #Packets) and
//              "Percentiles" (Dir, p50, p90, p99, p99.9). The tooltip texts of their header rows are shown on hover.
//
// ## Spec 39 (empty diagrams before any run; spec: .git/claude-specs/spec35-dashboard.md, "Spec 39")
//   Before any run (no history samples, state not running/starting, so also stopping/stopped/failed without a run) the chart area
//   is shown from localStorage: the Summary tab, then one link tab per STORED link in stored order, titled "name: ingress →
//   egress" (netdev labels, same fallbacks as live), no ended badge. No stored links: only the Summary tab (a tablist with one tab
//   and one tabpanel). Every tab: the Rate chart with no points (datasets RX, TX with empty data, axes titled, legend on), the radios
//   Traffic rates / Delay histogram / Packet loss with "Delay histogram" present but disabled on link tabs too, and the grey "No drops" donut with
//   the six-cause legend. getDelayHistogram is never called. While "starting" there are no tabs (as before); a running or ended run
//   takes its tabs from the run (live stats / history), never from the stored links.
//
// ## Spec 51 (IAT histogram, frame size donut, P4TG's rate and loss tables; spec: .git/claude-specs/spec51-iat-frame-sizes.md)
//   order      in a link's tabpanel and in the Summary's, top to bottom: the chart, the radiogroup "Chart", [Delay histogram view:
//              the three histogram tables above the delay bar chart], the rate table row, the loss table row, the donut row.
//   rate row   `.row.mb-3` > `.col-12.col-md-6.col-sm-12` > `table.table.table-striped.table-bordered.table-hover.table-sm.mt-3.mb-3`,
//              `thead.table-dark`, headers TX L1 | RX L1 | TX L2 | RX L2 (`th.col-3`), one body row (see rateTable()).
//   loss row   `.row` > `.col-12.col-sm-12.col-md-4` > the same table style, headers Lost Frames | Frame Loss Ratio | Out of Order.
//   donut row  (spec 52 R1: was three columns with the IAT histogram) ONE `.row` whose two children are `.col-12.col-md-6`: the loss
//              donut (left, its column also has the class `donut-divider`) and the frame size donut (right). Each chart sits in an
//              element with role "img", found by its aria-label (the contract of the helpers below): "Distribution of the dropped
//              packets by cause" (loss), "Distribution of the received frames by size" (frame sizes, CHOICE), "IAT histogram of
//              link <dir>" (IAT, like "Delay histogram of link <dir>"). Several `chart-doughnut` / `chart-bar` stubs can share a
//              panel: tests pick one through lossDonut(), frameDonut(), delayBar() or iatBar(), never by test id alone.
//
// ## Spec 52 (donut row in two centred columns, IAT histogram as a chart radio; spec: .git/claude-specs/spec52-donut-row-iat-radio.md)
//   radios     "Traffic rates", "Delay histogram", "IAT histogram", "Packet loss" (CHART_RADIOS), in this order; "IAT histogram" is present,
//              enabled and disabled exactly where "Delay histogram" is (delay statistics on, also after the stop; disabled in the
//              Summary and before any run; absent for a link without delay statistics). The Summary has no IAT placeholder text.
//   IAT view   chosen "IAT histogram": the three histogram tables (histTables(), headers IAT_TABLE_HEADERS) above the bar chart,
//              iatBar() in a `.chart-box`; polls getIatHistogram(dir) only while that radio is chosen and the tab is shown.
//
// ## Helpers
// `tabs()`, `tab(d)`, `hasTab(d)`, `card(d)` and the other helpers below address the LINK tabs only (the Summary tab is not counted),
// so the indexes of the existing tests are unchanged. `summaryTab()`, `summary()` and `summaryRadio()` address the Summary tab.
// `card(d)` is what `screen.getByTestId('direction-card-<d>')` was: it selects the tab of link position d (a no-op when it is
// selected) and returns that tab's tabpanel (found by aria-labelledby; other panels may stay mounted), so `within(card(d))` queries that link's chart area. Positions are the link indexes
// of every test here (0, 1, 2 ...), as the tabs come in index order.
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import type { EmulationState } from '../api/types';
import { must } from './must';

/** The title of the Summary tab (spec 36). */
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

/** The labels of the role "img" boxes around the donuts and the bar charts (spec 51). */
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
/** The radios of a chart area that has a histogram, in order (spec 52 R2). A link without delay statistics has the first and the last only. */
export const CHART_RADIOS = ['Traffic rates', 'Delay histogram', 'IAT histogram', 'Packet loss'];
/** Spec 52 R2b: the headers of the IAT view's three tables (the delay ones with "IAT" in place of "Delay"). */
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

/** Spec 51 T2: the headers of the two tables that are not histogram tables. */
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

/** The histogram tables of spec 36 in `scope`: every table that is neither the rate table nor the loss table. */
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
 * Cause labels of the donut and of the Packet loss chart's datasets, in the order of the donut's segments (spec 35 F5, spec 47 F1:
 * nine causes). Spec 50 B1: no counter names in the labels, they are in the legend's hover help (CAUSE_HELP).
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
 * The legend tooltips, exactly (spec 35 F5, spec 47 F1; spec 50 B2). CHOICE for B2, where the spec only shows "… (tx_drops)": the
 * counter names (comma and space separated when there are two) go in brackets right before the final full stop of the old text.
 * The first three causes have no counter and keep their text.
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
 * The run buttons per state (spec 43 F3/F8: the state badge is gone, and the buttons are P4TG's): Stop then Restart while
 * starting/running, the same two (disabled, Stop reading "Draining…") while stopping, only Start otherwise.
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
