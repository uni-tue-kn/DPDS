// Spec 50 C (user 2026-11-05: "new buttons in the settings are in the standard blue color of React. Do them in the color schema of
// DPDS"): the Delay accuracy ToggleButtons (`variant="outline-primary"`, ConfigFieldset.tsx) and the other outline-primary buttons
// take DPDS's red accent --color-primary (#e74c3c) through Bootstrap's --bs-btn-* variables, like the active sidebar link. jsdom
// renders no CSS, so the stylesheet itself is pinned. Bootstrap's own `.btn-check:checked + .btn`, `:hover`, `:active` and
// `:focus-visible` rules read these variables, so setting them on `.btn-outline-primary` covers every state; a rule of the
// implementation that sets them on a state selector of `.btn-outline-primary` is accepted as well (the last declaration wins).
// No Bootstrap blue (#0d6efd, rgb 13, 110, 253) may remain in the app's stylesheet.
// Spec 50 E (user 2026-11-05: "make the diagram for the rate, histogram and so on bigger (higher). They are currently really dense"):
// `.chart-box`, which holds every line and bar chart, is 250px high (user: 400px was too high, then 1.25 x 200px); the donut keeps its 220px.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { must } from '../test/must';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** The declarations of every rule with a selector in its list that mentions `.btn-outline-primary`, in source order. */
function outlinePrimary(): string {
  const re = /([^{}]+)\{([^{}]*)\}/g;
  const out: string[] = [];
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (m[1].split(',').some((s) => s.includes('.btn-outline-primary'))) out.push(m[2]);
  }
  return out.join('\n');
}

/** The value `property` gets in `declarations` (the last one wins), or undefined. */
function value(declarations: string, property: string): string | undefined {
  const re = new RegExp(`(?:^|[;\\s{])${property.replace(/[-]/g, '\\-')}\\s*:\\s*([^;]+)`, 'g');
  let last: string | undefined;
  for (let m = re.exec(declarations); m !== null; m = re.exec(declarations)) last = m[1].trim();
  return last;
}

const WHITE = /^(#fff|#ffffff|white)$/i;

describe('outline-primary buttons in the DPDS colours (spec 50 C)', () => {
  it('there is a rule for .btn-outline-primary', () => {
    expect(outlinePrimary()).not.toBe('');
  });

  it.each(['--bs-btn-color', '--bs-btn-border-color'])('%s is the DPDS red', (name) => {
    expect(value(outlinePrimary(), name)).toBe('var(--color-primary)');
  });

  it.each(['--bs-btn-hover-bg', '--bs-btn-hover-border-color', '--bs-btn-active-bg', '--bs-btn-active-border-color'])(
    '%s (hover, pressed and checked) is the DPDS red',
    (name) => {
      expect(value(outlinePrimary(), name)).toBe('var(--color-primary)');
    },
  );

  it.each(['--bs-btn-hover-color', '--bs-btn-active-color'])('%s is white on the red background', (name) => {
    expect(value(outlinePrimary(), name) ?? '').toMatch(WHITE);
  });

  it('the disabled state is red too, not Bootstrap blue', () => {
    expect(value(outlinePrimary(), '--bs-btn-disabled-color')).toBe('var(--color-primary)');
    expect(value(outlinePrimary(), '--bs-btn-disabled-border-color')).toBe('var(--color-primary)');
  });

  it('the focus shadow is P4TG red rgba(231, 76, 60, 0.25), not Bootstrap blue', () => {
    const r = outlinePrimary();
    expect(r).toMatch(/231,\s*76,\s*60/);
    expect(r).not.toMatch(/13,\s*110,\s*253/);
    // either the rgb triplet variable Bootstrap builds its shadow from, or the whole box-shadow with the 0.25 alpha
    const triplet = value(r, '--bs-btn-focus-shadow-rgb');
    const shadow = value(r, '--bs-btn-focus-box-shadow');
    expect(
      triplet === '231, 76, 60' || /rgba\(231,\s*76,\s*60,\s*0?\.25\)/.test(shadow ?? ''),
      `focus: ${triplet} / ${shadow}`,
    ).toBe(true);
  });

  it('DPDS red is still #e74c3c', () => {
    expect(css).toMatch(/--color-primary:\s*#e74c3c;/);
  });
});

describe('no Bootstrap blue in the stylesheet (spec 50 C)', () => {
  it('#0d6efd does not appear anywhere in index.css, in any case', () => {
    expect(css).not.toMatch(/#0d6efd/i);
  });

  it('nor its rgb triplet 13, 110, 253 (Bootstrap focus shadows)', () => {
    expect(css).not.toMatch(/13,\s*110,\s*253/);
  });
});

describe('taller charts (spec 50 E)', () => {
  const flat = css.replace(/\s+/g, ' ');

  // Spec 58 S1 (P4TG): the line and bar charts size themselves by aspect ratio (6 and 4); .chart-box has no fixed height any more.
  it('.chart-box has no fixed height (spec 58 S1; was 250px)', () => {
    const rule = must(/\.chart-box \{([^}]*)\}/.exec(flat), 'the .chart-box rule')[1];
    expect(rule).not.toMatch(/(?:^|[;\s])(?:min-|max-)?height:/);
  });

  it('the donut keeps its size: .donut-box is 220px by 220px', () => {
    const rule = must(/\.donut-box \{([^}]*)\}/.exec(flat), 'the .donut-box rule')[1];
    expect(rule).toMatch(/\bwidth: 220px;/);
    expect(rule).toMatch(/(?:^|[;\s])height: 220px;/);
  });
});
