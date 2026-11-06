// Helpers of the Dashboard flow tests. The chart area is a react-bootstrap Tabs: the first tab is "Summary", then one tab per link
// in index order. Each tabpanel holds the chart (role "img" with an aria-label, a stub inside), the radiogroup "Chart", the rate and
// loss tables and the donuts. `card(d)` selects the tab of link d and returns its panel; `summary()` does the same for the Summary.
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import type { EmulationState } from '../api/types';
import { propsOf } from './chartProps';
import { must } from './must';

const isSummary = (t: HTMLElement) => (t.textContent ?? '').trim() === 'Summary';

/** The link tabs of the page, in order (the Summary tab is not one of them). */
export const tabs = () => screen.queryAllByRole('tab').filter((t) => !isSummary(t));

export const tab = (d: number): HTMLElement => must(tabs()[d], `tab ${d}`);

/** The tab titles, "ingress → egress" (netdev labels). */
export const tabTitles = () => tabs().map((t) => (t.textContent ?? '').trim());

function panelOf(t: HTMLElement): HTMLElement {
  const panel = screen
    .getAllByRole('tabpanel', { hidden: true })
    .find((p) => p.getAttribute('aria-labelledby') === t.id);
  return must(panel, `the tabpanel of tab ${t.id}`);
}

function select(t: HTMLElement): HTMLElement {
  if (t.getAttribute('aria-selected') !== 'true') fireEvent.click(t);
  return panelOf(t);
}

/** Selects link tab `d` and returns its panel. */
export const card = (d: number): HTMLElement => select(tab(d));

/** Selects the Summary tab and returns its panel. */
export const summary = (): HTMLElement => select(must(screen.queryAllByRole('tab').find(isSummary), 'the Summary tab'));

/** Waits until tab `d` exists, then selects it. */
export async function findCard(d: number): Promise<HTMLElement> {
  await waitFor(() => expect(tabs().length).toBeGreaterThan(d));
  return card(d);
}

/** The radio `name` of a panel. */
export const radioIn = (panel: HTMLElement, name: string) => within(panel).getByRole('radio', { name });

/** The `data` / `options` of the chart stub inside the role "img" box `label` of `panel`. */
export function chartIn(panel: HTMLElement, label: string | RegExp) {
  const el = within(within(panel).getByRole('img', { name: label })).getByTestId(/^chart-/);
  return {
    data: JSON.parse(el.getAttribute('data-chart') ?? 'null') as {
      labels: string[];
      datasets: { label?: string; data: (number | null)[] }[];
    },
    props: propsOf(el),
  };
}

export const hasChart = (panel: HTMLElement, label: string | RegExp) =>
  within(panel).queryByRole('img', { name: label }) !== null;

/** The cells of the body row of a table with the given header cells, whitespace normalised. */
export function bodyCells(panel: HTMLElement, firstHeader: string): string[] {
  const table = must(
    within(panel)
      .getAllByRole('table')
      .find((t) => within(t).queryAllByRole('columnheader')[0]?.textContent === firstHeader),
    `the table starting with ${firstHeader}`,
  );
  return Array.from(must(within(table).getAllByRole('row')[1]).children).map((c) =>
    (c.textContent ?? '').replace(/\s+/g, ' ').trim(),
  );
}

/** The accessible names of the run buttons, in order. */
export const runButtonNames = () =>
  within(screen.getByTestId('run-buttons'))
    .queryAllByRole('button')
    .map((b) => (b.textContent ?? '').replace(/\s+/g, ' ').trim());

const RUN_BUTTONS: Record<EmulationState, string[]> = {
  stopped: ['Start'],
  starting: ['Stop', 'Restart'],
  running: ['Stop', 'Restart'],
  stopping: ['Draining…', 'Restart'],
  failed: ['Start'],
};

/** Waits until the run buttons read like `state` (stopped and failed look alike, as do starting and running). */
export const findState = (state: EmulationState) =>
  waitFor(() => expect(runButtonNames()).toEqual(RUN_BUTTONS[state]), { timeout: 4000 });

export const runButton = (name: string) => screen.getByRole('button', { name });

/** The text of a top-row pill, whitespace normalised. */
export const pillText = (testId: string) => (screen.getByTestId(testId).textContent ?? '').replace(/\s+/g, ' ').trim();
