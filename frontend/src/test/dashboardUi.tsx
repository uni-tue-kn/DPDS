// Spec 35 (user 2026-11-05): the minimal, P4TG-like dashboard with a loss donut. Contract of the frontend side, as the
// tests pin it (spec: scratchpad/spec35-dashboard.md, "Frontend layout" F1 to F7). The tester's choices where the spec is silent
// are marked CHOICE here and in the tests.
//
// ## Layout, top to bottom
//   top row    data-testid="top-row", two columns like P4TG's Home.tsx:
//                data-testid="top-pills"   `col-8 text-start`: state-badge, pill-rx, pill-tx, pill-elapsed, pill-dropped (DOM order)
//                data-testid="run-buttons" the ButtonGroup (role "group") inside a `col-4 text-end` column: exactly two buttons,
//                                          always [Restart] [Start | Stop | Stopping… | Restarting…]
//   notices    data-testid="notices", a row after the top row: "Restart needed" (pending-restart), stale-indicator, stats-warning,
//              history-warning, and every role="alert" (action error, poll error, emulation error). Never inside the top row.
//   banner     LoadRunning (unchanged), anywhere below the top row
//   chart area a react-bootstrap Tabs: role "tablist", one role "tab" per link (index order), titled "name: ingress → egress"
//              (netdev labels), with the badge "Stopping" / "Stopped" / "Failed" inside the tab while the run has ended.
//              Inside the active role "tabpanel": the chart (role "img" with an aria-label), centred below it the radiogroup
//              "Chart" with the radios "Rate", "Delay histogram" (only with delay_stats and while live) and "Loss", in this order,
//              then the loss donut.
//   donut      chart-doughnut (the Doughnut stub of test/setup.ts), the React legend data-testid="donut-legend" (one listitem per
//              cause, each with a `legend-swatch` and a button "About <label>" whose tooltip is the explanation), or the text
//              "No drops" when all six counts are 0 (then neither chart nor legend).
//   log        only for state "failed" and a non-empty log_tail: a closed <details> whose <summary> reads "Show log" and that
//              holds data-testid="log-tail".
//   ports      no ports table on the dashboard; PortsTable (data-testid="ports-table", heading "NIC ports") is on /interfaces.
//
// ## Helpers
// `card(d)` is what `screen.getByTestId('direction-card-<d>')` was: it selects the tab of link position d (a no-op when it is
// selected) and returns that tab's tabpanel (found by aria-labelledby; other panels may stay mounted), so `within(card(d))` queries that link's chart area. Positions are the link indexes
// of every test here (0, 1, 2 ...), as the tabs come in index order.
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { must } from './must';

/** All tabs of the page, in order. */
export const tabs = () => screen.queryAllByRole('tab');

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
  const selected = tabs().filter((t) => t.getAttribute('aria-selected') === 'true');
  if (selected.length !== 1) throw new Error(`expected exactly one selected tab, found ${selected.length}`);
  return panelOf(selected[0]);
}

/** Selects tab `d` and returns its tabpanel (the one chart area that shows). */
export function card(d: number): HTMLElement {
  const t = tab(d);
  if (t.getAttribute('aria-selected') !== 'true') fireEvent.click(t);
  return panelOf(t);
}

/** Waits until there are `n` tabs; selects nothing. */
export async function findTabs(n: number): Promise<void> {
  await waitFor(() => expect(tabs().length).toBeGreaterThanOrEqual(n));
}

/** Waits until there is a tab `d`, then returns `card(d)` (which selects that tab). */
export async function findCard(d: number): Promise<HTMLElement> {
  await waitFor(() => expect(tabs().length).toBeGreaterThan(d));
  return card(d);
}

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

/** The visible text of a pill, whitespace normalised ("RX 9.25 Gbit/s (7.31 Mpps)"). */
export const pillText = (testId: string) => (screen.getByTestId(testId).textContent ?? '').replace(/\s+/g, ' ').trim();

/** Cause labels of the donut, in the order of its segments (F5). */
export const CAUSES = [
  'Loss model',
  'Rate limiter',
  'Buffer full',
  'NIC TX ring full (tx_drops)',
  'NIC RX missed (imissed)',
  'RX buffers exhausted (rx_nombuf)',
] as const;

/** The legend tooltips of F5, exactly. */
export const CAUSE_HELP: Record<(typeof CAUSES)[number], string> = {
  'Loss model': 'Packets dropped on purpose by the configured loss model (random or Gilbert-Elliott).',
  'Rate limiter': "Packets the limiter refused: the Spacer's backlog or the token bucket's budget was exceeded.",
  'Buffer full': 'Packets that arrived while the delay buffer already held its maximum number of packets.',
  'NIC TX ring full (tx_drops)': 'Packets the NIC would not accept for sending because its transmit ring was full.',
  'NIC RX missed (imissed)': 'Packets the NIC dropped because its receive queues were full.',
  'RX buffers exhausted (rx_nombuf)': 'Packets lost because no packet buffer (mbuf) was free to receive them.',
};
