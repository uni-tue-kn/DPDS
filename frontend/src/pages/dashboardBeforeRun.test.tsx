import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import { vi } from 'vitest';
import type { EmulationState, Link } from '../api/types';
import {
  activePanel,
  allTabs,
  CAUSES,
  CHART_RADIOS,
  card,
  hasSummary,
  lossDonut,
  panelOf,
  queryRadio,
  radioNames,
  summary,
  summaryTab,
  tab,
  tabBadge,
  tabs,
  tabTitle,
} from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore, tick } from '../test/helpers';
import { api, serveHistory } from '../test/mockApi';
import { must } from '../test/must';

// Before any run (no history samples, state not running/starting) the dashboard shows the empty diagrams (contract: header of
// src/test/dashboardUi.tsx):
//   - the Summary tab plus one tab per STORED link in stored order, titled "name: ingress → egress" with netdev names, no ended
//     badge; no stored links: the Summary only;
//   - every tab: the empty Rate chart (axes titled, legend on), the radios with "Delay histogram" present but disabled, the grey
//     "No drops" donut with the nine-cause legend;
//   - nothing polls getDelayHistogram;
//   - once a run starts the tabs come from the run; an ended run's tabs stay, the stored links only fill in the before-any-run view;
//   - while a run is starting the tabs a view has stay, the overlay covers them (dashboardSpec54.test.tsx); stopping/stopped/failed
//     without samples show the stored view.

const LINKS = [fx.link(), fx.linkRev()];

function serveEnded(state: EmulationState = 'stopped') {
  api.getEmulation.mockResolvedValue(
    fx.emulation(state, { directions: [], error: state === 'failed' ? 'dpds exited (status 1)' : null }),
  );
  api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
}

async function open(links: Link[] = LINKS) {
  seedStore({ links });
  renderApp('/');
  await screen.findByRole('tablist');
}

interface LineData {
  labels: string[];
  datasets: { label: string; data: number[] }[];
}
const line = (panel: HTMLElement): LineData =>
  JSON.parse(within(panel).getByTestId('chart-line').getAttribute('data-chart') ?? 'null');

/** The chart area of the Summary (d undefined) or of link tab d. */
const panelFor = (d?: number) => (d === undefined ? summary() : card(d));
const VIEWS: [string, number | undefined][] = [
  ['the Summary tab', undefined],
  ['link tab 0', 0],
  ['link tab 1', 1],
];

describe('Dashboard before any run (spec 39): the tabs', () => {
  it('shows the Summary tab, selected, then one tab per stored link in stored order, titled with netdev names and no badge', async () => {
    await open();
    expect(allTabs()).toHaveLength(3);
    expect(allTabs()[0]).toBe(summaryTab());
    expect(summaryTab()).toHaveAttribute('aria-selected', 'true');
    expect(activePanel()).toBe(panelOf(summaryTab()));
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabTitle(1)).toBe('b-a: enp101s0f1 → enp101s0f0');
    expect(tabBadge(0)).toBe('');
    expect(tabBadge(1)).toBe('');
    expect(screen.queryByText(/^(Stopped|Stopping|Failed)$/)).not.toBeInTheDocument();
  });

  it('follows the stored order, not the id or name order', async () => {
    await open([fx.linkRev(), fx.link()]);
    await waitFor(() => expect(tabTitle(0)).toBe('b-a: enp101s0f1 → enp101s0f0'));
    expect(tabTitle(1)).toBe('a-b: enp101s0f0 → enp101s0f1');
  });

  it('falls back to the stored port names, and to "?", as the live titles do', async () => {
    await open([fx.link({ name: 'only', ingress: 'net_null0', egress: 'net_null1' })]);
    await waitFor(() => expect(tabs()).toHaveLength(1));
    expect(tabTitle(0)).toBe('only: net_null0 → net_null1');
  });

  it('without stored links there is only the Summary tab, with its chart area', async () => {
    await open([]);
    expect(allTabs()).toHaveLength(1);
    expect(hasSummary()).toBe(true);
    expect(tabs()).toHaveLength(0);
    expect(screen.getAllByRole('tablist')).toHaveLength(1);
    expect(within(summary()).getByTestId('chart-line')).toBeInTheDocument();
    expect(lossDonut(summary())).toBeInTheDocument();
  });

  it.each(['stopped', 'stopping', 'failed'] as const)(
    'also in the state "%s" when no run left samples',
    async (state) => {
      serveEnded(state);
      await open();
      expect(allTabs()).toHaveLength(3);
      await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
      expect(tabBadge(0)).toBe('');
    },
  );

  it('a page opened while the run is starting shows the loading overlay (spec 54 L3), not an empty page', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    api.getStats.mockResolvedValue(fx.stats([], { stale: true, age_ms: undefined }));
    seedStore({ links: LINKS });
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    expect(document.querySelector('.loader-overlay')).not.toBeNull();
  });

  it('a running emulation takes its tabs from the run, not from the stored links', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0)]));
    seedStore({ links: [...LINKS, fx.loopLink()] });
    renderApp('/');
    await waitFor(() => expect(tabs()).toHaveLength(1));
    expect(hasSummary()).toBe(true);
  });

  it('an ended run keeps its own tabs, unbadged (spec 54 L1), whatever is stored', async () => {
    serveHistory({ startedAt: 1762000000, samples: { '0': fx.historySamples(0, 3) } });
    serveEnded('stopped');
    seedStore({ links: [...LINKS, fx.loopLink()] });
    renderApp('/');
    await waitFor(() => expect(tabs()).toHaveLength(1));
    await waitFor(() => expect(tabTitle(0)).toBe('a-b: enp101s0f0 → enp101s0f1'));
    expect(tabBadge(0)).toBe('');
    expect(tabs()).toHaveLength(1);
  });

  it('a new start keeps them: the tabs stay while starting', async () => {
    await open();
    expect(tabs()).toHaveLength(2);
    api.getEmulation.mockResolvedValue(fx.emulation('starting'));
    await waitFor(() => expect(document.querySelector('.loader-overlay')).not.toBeNull(), { timeout: 4000 });
    expect(screen.getByRole('tablist')).toBeInTheDocument();
    expect(tabs()).toHaveLength(2);
  });
});

describe('Dashboard before any run (spec 39): the empty diagrams', () => {
  it.each(VIEWS)('%s, the Rate chart is there, empty, with titled axes and the legend', async (_name, d) => {
    await open();
    const panel = panelFor(d);
    expect(within(panel).getByRole('img', { name: /^(Rate|Traffic rates) chart/ })).toBeInTheDocument();
    const data = line(panel);
    expect(data.labels).toEqual([]);
    expect(data.datasets.map((s) => s.label)).toEqual(['TX rate', 'RX rate']);
    for (const s of data.datasets) expect(s.data).toEqual([]);
    const options = JSON.parse(within(panel).getByTestId('chart-line').getAttribute('data-options') ?? 'null');
    expect(options.scales.x.title.text).toBe('Time');
    expect(options.scales.y.title.text).toBe('Gbit/s');
    expect(options.plugins.legend.display).not.toBe(false);
  });

  it.each(VIEWS)('%s, Rate is checked, "Delay histogram" is disabled, Loss works and is empty', async (_name, d) => {
    await open();
    const panel = panelFor(d);
    expect(within(panel).getByRole('radiogroup', { name: 'Chart' })).toBeInTheDocument();
    const names = within(panel)
      .getAllByRole('radio')
      .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim());
    expect(names).toEqual(CHART_RADIOS);
    expect(within(panel).getByRole('radio', { name: 'Traffic rates' })).toBeChecked();
    expect(within(panel).getByRole('radio', { name: 'Delay histogram' })).toBeDisabled();
    expect(within(panel).getByRole('radio', { name: 'Packet loss' })).toBeEnabled();
    fireEvent.click(within(panel).getByRole('radio', { name: 'Packet loss' }));
    expect(within(panel).getByRole('img', { name: /^(Loss|Packet loss) chart/ })).toBeInTheDocument();
    const data = line(panel);
    expect(data.datasets.map((s) => s.label)).toEqual([...CAUSES]); // one line per cause
    for (const s of data.datasets) expect(s.data).toEqual([]);
  });

  it('a link tab has the same radios as a live link without delay statistics plus the disabled histogram', async () => {
    await open();
    expect(radioNames(0)).toEqual(CHART_RADIOS);
    expect(must(queryRadio(0, 'Delay histogram'), 'Delay histogram radio')).toBeDisabled();
  });

  it.each(VIEWS)('%s, the donut is the grey "No drops" ring with the nine-cause legend', async (_name, d) => {
    await open();
    const panel = panelFor(d);
    const donut = JSON.parse(lossDonut(panel).getAttribute('data-chart') ?? 'null');
    expect(donut.datasets).toHaveLength(1);
    expect(donut.datasets[0].data).toHaveLength(1);
    expect(donut.datasets[0].data[0]).toBeGreaterThan(0);
    expect(within(panel).getByText('No drops')).toBeInTheDocument();
    const items = within(within(panel).getByTestId('donut-legend')).getAllByRole('listitem');
    expect(items).toHaveLength(9);
    CAUSES.forEach((cause, i) => {
      expect(items[i], `${name}: ${cause}`).toHaveTextContent(cause);
    });
  });

  it('nothing polls getDelayHistogram, whichever tab is chosen', async () => {
    seedStore({ links: LINKS });
    vi.useFakeTimers();
    renderApp('/');
    await tick(0);
    await tick(0);
    fireEvent.click(tab(0));
    await tick(3000);
    fireEvent.click(tab(1));
    await tick(3000);
    expect(api.getDelayHistogram).not.toHaveBeenCalled();
  });
});
