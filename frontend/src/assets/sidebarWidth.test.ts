// User (2026-11-05): "make the left sidebar as wide as P4TG's". P4TG (Configuration GUI/src/App.tsx:146, commit fcf24bd)
// puts its navbar in `col-2 col-sm-2 col-xl-1`: 1/6 of the width below 1200 px, 1/12 from 1200 px (Bootstrap's xl), and its
// logo is 80 px wide (components/Navbar.tsx StyledImg). jsdom renders no CSS, so the stylesheet itself is pinned.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

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

  it('shows the logo 80 px wide, shrinking only when the sidebar is narrower', () => {
    expect(css).toMatch(/\.sidebar-brand img \{[^}]*\bwidth: 80px;[^}]*\bmax-width: 100%;/);
  });
});
