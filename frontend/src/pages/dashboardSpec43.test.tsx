import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen, within } from '@testing-library/react';
import { card, CAUSES, findCard, summary } from '../test/dashboardUi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

// Spec 43 (user 2026-11-05), the frontend parts that have no home in an older test file:
//   F4  the RX pill's arrow is always the yellow/orange one (`--color-yellow`), also while running; the TX pill's arrow is green
//       (`--color-okay`) while running and yellow otherwise. jsdom renders no CSS, so the colour that applies to the arrow `<i>` is
//       worked out from the stylesheet: every top-level rule of index.css whose selector matches the element (or an ancestor, for
//       inheritance), the one with the highest specificity (later wins a tie) sets `color`. This does not care which class names the
//       implementation uses. CHOICE: the value is `var(--color-yellow)` / `var(--color-okay)` or the literal hex of the variable.
//   F5  the radios of a link's chart area are "Traffic rates", "Packet loss", "Delay histogram"; the old names are gone.
// (F1 to F3, F6 and F8 are in dashboardP4tg, dashboardSpec35, dashboardSummary, dashboardHistory, interfacesNoStats; F7 in
// src/assets/stylesSpec43.test.ts.)

const css = readFileSync(resolve(__dirname, '../assets/index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** `--name: value` of the first `:root` rule that defines it. */
const variable = (name: string) =>
  must(new RegExp(`${name}\\s*:\\s*([^;]+);`).exec(css), `variable ${name}`)[1]
    .trim()
    .toLowerCase();

function specificity(selector: string): number {
  const ids = (selector.match(/#[\w-]+/g) ?? []).length;
  const classes = (selector.match(/\.[\w-]+|\[[^\]]*\]|:(?!:)[\w-]+(?:\([^)]*\))?/g) ?? []).length;
  const bare = selector.replace(/\.[\w-]+|#[\w-]+|\[[^\]]*\]|:+[\w-]+(?:\([^)]*\))?/g, ' ');
  const tags = (bare.match(/(?:^|[\s>+~])[a-zA-Z][\w-]*/g) ?? []).length;
  return ids * 10000 + classes * 100 + tags;
}

interface Decl {
  weight: number;
  order: number;
  value: string;
}

/** The `color` declared for `el` itself by the stylesheet (highest specificity, then last), or undefined. */
function ownColor(el: Element): string | undefined {
  const found: Decl[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let order = 0;
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    order += 1;
    const color = /(?:^|[;\s])color\s*:\s*([^;]+)/.exec(m[2]);
    if (!color || m[1].trim().startsWith('@')) continue;
    for (const selector of m[1].split(',').map((s) => s.trim())) {
      let hit = false;
      try {
        hit = el.matches(selector);
      } catch {
        hit = false;
      }
      if (hit) found.push({ weight: specificity(selector), order, value: color[1].trim().toLowerCase() });
    }
  }
  found.sort((a, b) => a.weight - b.weight || a.order - b.order);
  return found.at(-1)?.value;
}

/** The colour of the arrow icon in `pill`, inherited from an ancestor if no rule hits the icon. */
function arrowColor(pill: HTMLElement): string {
  const icon = must(pill.querySelector('i'), 'the arrow icon');
  for (let el: Element | null = icon; el; el = el.parentElement) {
    const c = ownColor(el);
    if (c) return c;
  }
  throw new Error('no colour rule applies to the arrow');
}

const isVariable = (colour: string, name: '--color-yellow' | '--color-okay') =>
  colour === `var(${name})` || colour === variable(name);

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

  it('while running the RX arrow is yellow and the TX arrow green', async () => {
    const { rx, tx } = await arrows('running');
    expect(isVariable(arrowColor(rx), '--color-yellow'), `RX arrow: ${arrowColor(rx)}`).toBe(true);
    expect(isVariable(arrowColor(tx), '--color-okay'), `TX arrow: ${arrowColor(tx)}`).toBe(true);
  });

  it.each(['stopped', 'stopping', 'failed', 'starting'] as const)('"%s": both arrows are yellow', async (state) => {
    const { rx, tx } = await arrows(state);
    expect(isVariable(arrowColor(rx), '--color-yellow'), `RX arrow: ${arrowColor(rx)}`).toBe(true);
    expect(isVariable(arrowColor(tx), '--color-yellow'), `TX arrow: ${arrowColor(tx)}`).toBe(true);
  });

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
    expect(names(card(0))).toEqual(['Traffic rates', 'Delay histogram', 'Packet loss']);
  });

  it('the Summary tab has the same three', async () => {
    renderApp('/');
    await findCard(1);
    expect(names(summary())).toEqual(['Traffic rates', 'Delay histogram', 'Packet loss']);
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
