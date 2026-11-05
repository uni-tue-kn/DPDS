// User (2026-11-05): "make the left sidebar as wide as P4TG's". P4TG (Configuration GUI/src/App.tsx:146, commit fcf24bd)
// puts its navbar in `col-2 col-sm-2 col-xl-1`: 1/6 of the width below 1200 px, 1/12 from 1200 px (Bootstrap's xl), and its
// logo is 80 px wide (components/Navbar.tsx StyledImg). jsdom renders no CSS, so the stylesheet itself is pinned.
// Spec 50 D (user 2026-11-05: "Make the DPDS logo in the top left bigger. It is really small right now."): the logo (256x97) is
// no longer 80 px wide but fills the sidebar's width, the brand keeping at most 0.25rem of horizontal padding.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { must } from '../test/must';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8')
  .replace(/\/\*[\s\S]*?\*\//g, '')
  .replace(/\s+/g, ' ');

describe('sidebar width like P4TG', () => {
  it('is 1/6 of the width by default', () => {
    expect(css).toMatch(/\.sidebar \{[^}]*\bwidth: 16\.6667%;/);
    expect(css).not.toMatch(/\.sidebar \{[^}]*\bwidth: 72px;/);
  });

  it('is 1/12 of the width from 1200 px', () => {
    expect(css).toMatch(/@media \(min-width: 1200px\) \{ \.sidebar \{ width: 8\.3333%; \} \}/);
  });

  it('shows the logo as wide as the sidebar (spec 50 D), not 80 px', () => {
    expect(css).toMatch(/\.sidebar-brand img \{[^}]*\bwidth: 100%;[^}]*\bmax-width: 100%;/);
    expect(css).not.toMatch(/\.sidebar-brand img \{[^}]*\bwidth: 80px;/);
  });

  it('the brand pads the logo by at most 0.25rem on the left and the right (spec 50 D)', () => {
    const rule = /\.sidebar-brand \{([^}]*)\}/.exec(css);
    const body = must(rule, 'the .sidebar-brand rule')[1];
    const px = (v: string) => (v.endsWith('rem') ? Number.parseFloat(v) * 16 : Number.parseFloat(v));
    const shorthand = /(?:^|[;\s])padding: ([^;]+);/.exec(body);
    const parts = shorthand ? shorthand[1].trim().split(' ') : [];
    const side = (name: 'left' | 'right') => {
      const own = new RegExp(`padding-${name}: ([^;]+);`).exec(body);
      if (own) return px(own[1].trim());
      if (parts.length === 1) return px(parts[0]);
      if (parts.length === 4) return px(parts[name === 'right' ? 1 : 3]);
      return px(parts[1] ?? '0');
    };
    expect(side('left')).toBeLessThanOrEqual(4);
    expect(side('right')).toBeLessThanOrEqual(4);
  });

  // P4TG index.scss:270-280 and :834-837: navigation icons are 25px, 20px up to 450 px width (user: DPDS's were "a bit tiny").
  it('navigation icons are 25px like P4TG, 20px on screens up to 450 px', () => {
    expect(css).toMatch(/\.sidebar \.nav-link \{[^}]*\bfont-size: 25px;/);
    expect(css).toMatch(/@media \(max-width: 450px\) \{ \.sidebar \.nav-link \{ font-size: 20px; \} \}/);
  });
});
