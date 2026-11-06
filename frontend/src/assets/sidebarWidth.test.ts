// The sidebar is as wide as P4TG's (1/6 of the width below 1200 px, 1/12 from 1200 px, Bootstrap's xl). jsdom renders no CSS, so the stylesheet itself is pinned.
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

  it('has no styles for a sidebar brand any more (the logo is the page title)', () => {
    expect(css).not.toMatch(/\.sidebar-brand/);
  });

  // Navigation icons are 25px, 20px up to 450 px width.
  it('navigation icons are 25px like P4TG, 20px on screens up to 450 px', () => {
    expect(css).toMatch(/\.sidebar \.nav-link \{[^}]*\bfont-size: 25px;/);
    expect(css).toMatch(/@media \(max-width: 450px\) \{ \.sidebar \.nav-link \{ font-size: 20px; \} \}/);
  });
});
