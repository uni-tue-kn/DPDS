// User 2026-11-05: the Start, Stop and Restart buttons in "the exact colours P4TG uses". P4TG (Configuration GUI
// src/assets/index.scss, Bootstrap 5.3.8 like DPDS) overrides `.btn-primary` and `.btn-primary:disabled` to
// --color-okay #27ae60 (hover --color-okay-hover #20864b) and `.btn-danger` to background --color-primary #e74c3c (its border,
// hover and disabled colours stay Bootstrap's). jsdom renders no CSS, so the stylesheet itself is pinned.
// CHOICE: DPDS sets the colours through Bootstrap's --bs-btn-* variables; the pressed (active) primary uses the hover colour.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** All declarations of every rule whose selector list includes `selector`. */
function rules(selector: string): string {
  const norm = (s: string) => s.replace(/\s+/g, ' ').trim();
  const re = /([^{}]+)\{([^{}]*)\}/g;
  const out: string[] = [];
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (m[1].split(',').map(norm).includes(norm(selector))) out.push(m[2]);
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

describe('button colours like P4TG', () => {
  it('the colour variables have P4TG values', () => {
    const root = rules(':root');
    expect(value(root, '--color-okay')).toBe('#27ae60');
    expect(value(root, '--color-okay-hover')).toBe('#20864b');
    expect(value(root, '--color-primary')).toBe('#e74c3c');
  });

  it.each(['.btn-primary', '.btn-primary:disabled'])('%s is green, its hover the darker green', (sel) => {
    const r = rules(sel);
    expect(value(r, '--bs-btn-bg')).toBe('var(--color-okay)');
    expect(value(r, '--bs-btn-border-color')).toBe('var(--color-okay)');
    expect(value(r, '--bs-btn-hover-bg')).toBe('var(--color-okay-hover)');
    expect(value(r, '--bs-btn-hover-border-color')).toBe('var(--color-okay-hover)');
    expect(value(r, '--bs-btn-active-bg')).toBe('var(--color-okay-hover)');
    expect(value(r, '--bs-btn-active-border-color')).toBe('var(--color-okay-hover)');
    expect(value(r, '--bs-btn-disabled-bg')).toBe('var(--color-okay)');
    expect(value(r, '--bs-btn-disabled-border-color')).toBe('var(--color-okay)');
  });

  it('.btn-danger has the P4TG red background and keeps Bootstrap’s border and hover', () => {
    const r = rules('.btn-danger');
    expect(value(r, '--bs-btn-bg')).toBe('var(--color-primary)');
    expect(value(r, '--bs-btn-border-color')).toBeUndefined();
    expect(value(r, '--bs-btn-hover-bg')).toBeUndefined();
  });

  it('no rule makes the primary button red any more', () => {
    expect(rules('.btn-primary')).not.toMatch(/--color-primary|#c0392b/);
  });
});
