import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen, within } from '@testing-library/react';
import { CAUSES, CHART_RADIOS, card, findCard, summary } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

// Spec 43 (user 2026-11-05), the frontend parts that have no home in an older test file:
//   F4  (amended by the user 2026-11-05: "no matter if the DPDS is started or stopped"; spec 46: P4TG order, no RX/TX label) the
//       RX pill's arrow is always the yellow/orange one (`--color-yellow`) and the TX pill's arrow always the green one
//       (`--color-okay`), in every state. Spec 46 sets them inline on the arrow `<i>` (`style.color`); CHOICE: see the header of
//       dashboardTopRow.test.tsx. Was: the colour worked out from the rules of index.css (specificity walk). The full set of
//       top row tests is in dashboardTopRow.test.tsx.
//   F5  the radios of a link's chart area are "Traffic rates", "Packet loss", "Delay histogram"; the old names are gone.
// (F1 to F3, F6 and F8 are in dashboardP4tg, dashboardSpec35, dashboardSummary, dashboardHistory, interfacesNoStats; F7 in
// src/assets/stylesSpec43.test.ts.)

const css = readFileSync(resolve(__dirname, '../assets/index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** `--name: value` of the first `:root` rule that defines it. */
const variable = (name: string) =>
  must(new RegExp(`${name}\\s*:\\s*([^;]+);`).exec(css), `variable ${name}`)[1]
    .trim()
    .toLowerCase();

async function arrows(state: 'running' | 'stopped' | 'stopping' | 'failed' | 'starting') {
  seedStore({ links: [fx.link(), fx.linkRev()] });
  api.getEmulation.mockResolvedValue(fx.emulation(state));
  api.getStats.mockResolvedValue(
    state === 'running' ? fx.stats([fx.dirStats(0), fx.dirStats(1)]) : fx.stats([], { stale: true, age_ms: undefined }),
  );
  renderApp('/');
  const rx = await screen.findByTestId('pill-rx');
  const tx = await screen.findByTestId('pill-tx');
  return { rx, tx };
}

describe('Dashboard (spec 43 F4): the arrows of the RX and TX pills', () => {
  it('the colour variables the arrows use exist: --color-yellow and --color-okay', () => {
    expect(variable('--color-yellow')).toMatch(/^#[0-9a-f]{6}$/);
    expect(variable('--color-okay')).toMatch(/^#[0-9a-f]{6}$/);
  });

  it.each(['running', 'stopped', 'stopping', 'failed', 'starting'] as const)(
    '"%s": the RX arrow is yellow and the TX arrow green',
    async (state) => {
      const { rx, tx } = await arrows(state);
      expect(must(rx.querySelector('i')).style.color).toBe('var(--color-yellow)');
      expect(must(tx.querySelector('i')).style.color).toBe('var(--color-okay)');
    },
  );

  it('the arrows keep their icons (RX down, TX up)', async () => {
    const { rx, tx } = await arrows('running');
    expect(must(rx.querySelector('i'))).toHaveClass('bi-arrow-down-circle-fill');
    expect(must(tx.querySelector('i'))).toHaveClass('bi-arrow-up-circle-fill');
  });
});

describe('Dashboard (spec 43 F5): the radio names', () => {
  beforeEach(() => {
    seedStore({ links: [fx.link(), fx.linkRev()] });
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    api.getStats.mockResolvedValue(fx.stats([fx.dirStats(0), fx.dirStats(1)]));
  });

  const names = (panel: HTMLElement) =>
    within(panel)
      .getAllByRole('radio')
      .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim());

  it('a link tab: "Traffic rates", "Delay histogram", "Packet loss", in this order', async () => {
    renderApp('/');
    await findCard(1);
    expect(names(card(0))).toEqual(CHART_RADIOS);
  });

  it('the Summary tab has the same three', async () => {
    renderApp('/');
    await findCard(1);
    expect(names(summary())).toEqual(CHART_RADIOS);
  });

  it('the old names "Rate" and "Loss" are not used for any radio', async () => {
    renderApp('/');
    await findCard(1);
    for (const panel of [card(0), card(1), summary()]) {
      expect(within(panel).queryByRole('radio', { name: 'Rate' })).not.toBeInTheDocument();
      expect(within(panel).queryByRole('radio', { name: 'Loss' })).not.toBeInTheDocument();
    }
  });

  it('the donut legend keeps its cause names ("Loss model" is not a radio)', async () => {
    renderApp('/');
    await findCard(1);
    const legend = within(card(0)).getByTestId('donut-legend');
    expect(within(legend).getAllByRole('listitem')).toHaveLength(CAUSES.length);
  });
});
