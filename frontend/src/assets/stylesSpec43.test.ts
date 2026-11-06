// The active sidebar link's red block has rounded corners (0.375rem) and the links are inset 0.5rem from the sidebar edges, for the
// active and the hover state alike. jsdom renders no CSS, so the stylesheet itself is pinned. The radius is on the base rule
// `.sidebar .nav-link`, so hover and active share it (their own rules may not take it away: no `border-radius: 0`, no other radius);
// the inset is a horizontal margin of 0.5rem on `.sidebar .nav-link` (`margin`, `margin-inline`, or `margin-left` and `margin-right`),
// or a horizontal padding of 0.5rem on `.sidebar` itself (the nav links are its direct children).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** The declarations of every rule whose selector list includes `selector`, one string per rule. */
function rules(selector: string): string[] {
  const norm = (s: string) => s.replace(/\s+/g, ' ').trim();
  const re = /([^{}]+)\{([^{}]*)\}/g;
  const out: string[] = [];
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (m[1].split(',').map(norm).includes(norm(selector))) out.push(m[2]);
  }
  return out;
}

const REM = '0?\\.5rem';

/** True iff `declarations` give both horizontal sides of `property` (margin or padding) the value 0.5rem. */
function insetsHorizontally(declarations: string, property: 'margin' | 'padding'): boolean {
  const short = new RegExp(`(?:^|[;\\s])${property}\\s*:\\s*([^;]+)`, 'g');
  let last: string[] | undefined;
  for (let m = short.exec(declarations); m !== null; m = short.exec(declarations)) last = m[1].trim().split(/\s+/);
  if (last) {
    const right = last.length === 1 ? last[0] : last[1];
    const left = last.length === 4 ? last[3] : right;
    if (new RegExp(`^${REM}$`).test(left) && new RegExp(`^${REM}$`).test(right)) return true;
  }
  if (new RegExp(`${property}-inline\\s*:\\s*${REM}\\s*(?:;|$)`).test(declarations)) return true;
  const l = new RegExp(`${property}-left\\s*:\\s*${REM}\\s*(?:;|$)`).test(declarations);
  const r = new RegExp(`${property}-right\\s*:\\s*${REM}\\s*(?:;|$)`).test(declarations);
  return l && r;
}

describe('sidebar links (spec 43 F7)', () => {
  const link = () => rules('.sidebar .nav-link').join('\n');

  it('`.sidebar .nav-link` has border-radius 0.375rem', () => {
    expect(link()).toMatch(/border-radius:\s*0?\.375rem/);
  });

  it('the hover and the active rules do not take the radius away', () => {
    for (const selector of ['.sidebar .nav-link:hover', '.sidebar .nav-link.active']) {
      for (const body of rules(selector)) {
        const radius = /border-radius:\s*([^;]+)/.exec(body);
        if (radius) expect(radius[1].trim(), selector).toMatch(/^0?\.375rem$/);
      }
    }
  });

  it('the active link is still the solid red block with white text', () => {
    const active = rules('.sidebar .nav-link.active').join('\n');
    expect(active).toMatch(/background(?:-color)?:\s*var\(--color-primary\)/);
    expect(active).toMatch(/color:\s*(?:#fff(?:fff)?\b|white)/i);
  });

  it('the links are inset 0.5rem from the sidebar edges (a margin on the link or a padding on the sidebar)', () => {
    const onLink = insetsHorizontally(link(), 'margin');
    const onSidebar = rules('.sidebar').some((body) => insetsHorizontally(body, 'padding'));
    expect(onLink || onSidebar).toBe(true);
  });

  it('the hover and the active rules do not zero the inset again', () => {
    for (const selector of ['.sidebar .nav-link:hover', '.sidebar .nav-link.active']) {
      for (const body of rules(selector))
        expect(body, selector).not.toMatch(/(?:^|[;\s])margin(?:-left|-right|-inline)?\s*:\s*0\s*(?:;|$)/);
    }
  });
});
