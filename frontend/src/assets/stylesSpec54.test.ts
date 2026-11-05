// Spec 54 L2 (user 2026-11-05: loading overlay like P4TG's): the Loader's styles are CSS classes in index.css, ported from the styled
// components of P4TG's Loader.tsx, and the colour token --color-background-transparent exists for the light and the dark theme.
// jsdom renders no CSS, so the stylesheet itself is pinned. Class names: see the header of components/loader.test.tsx (CHOICE).
// The wordmark `.loader-text` is bold and in the primary colour; its size is the loader's 2em, inherited (it must not scale it again).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

/** The declarations of every rule whose selector list has exactly `selector` as an item, in source order. */
function rules(selector: string): string {
  const re = /([^{}]+)\{([^{}]*)\}/g;
  const out: string[] = [];
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (m[1].split(',').some((s) => s.trim().replace(/\s+/g, ' ') === selector)) out.push(m[2]);
  }
  return out.join('\n');
}

/** The value `property` gets in `declarations` (the last one wins, whitespace normalised), or undefined. */
function value(declarations: string, property: string): string | undefined {
  const re = new RegExp(`(?:^|[;\\s{])${property.replace(/-/g, '\\-')}\\s*:\\s*([^;]+)`, 'g');
  let last: string | undefined;
  for (let m = re.exec(declarations); m !== null; m = re.exec(declarations)) last = m[1].trim().replace(/\s+/g, ' ');
  return last;
}

const ZERO = /^0(px)?$/;

describe('the Loader classes (spec 54 L2, P4TG Loader.tsx)', () => {
  it.each([
    '.loader-content',
    '.loader-block',
    '.loader-overlay',
    '.loader-spinner-box',
    '.loader-spinner',
    '.loader-text',
  ])('%s has a rule', (selector) => {
    expect(rules(selector)).not.toBe('');
  });

  it('.loader-content (ContentWrapper) is positioned, so the overlay lies over it', () => {
    expect(value(rules('.loader-content'), 'position')).toBe('relative');
  });

  it('.loader-block (LoadWrapper) is the centred flex block of the whole height, 2em, primary colour', () => {
    const r = rules('.loader-block');
    expect(value(r, 'display')).toBe('flex');
    expect(value(r, 'justify-content')).toBe('center');
    expect(value(r, 'align-items')).toBe('center');
    expect(value(r, 'flex-grow')).toBe('1');
    expect(value(r, 'font-size')).toBe('2em');
    expect(value(r, 'color')).toBe('var(--color-primary)');
    expect(value(r, 'text-align')).toBe('center');
    expect(value(r, 'height')).toBe('100%');
  });

  it('.loader-overlay (LoadWrapperAbsolute) covers the content: absolute, top 0, 100% x 100%, z-index 1, translucent background', () => {
    const r = rules('.loader-overlay');
    expect(value(r, 'position')).toBe('absolute');
    expect(value(r, 'top') ?? '').toMatch(ZERO);
    expect(value(r, 'width')).toBe('100%');
    expect(value(r, 'height')).toBe('100%');
    expect(value(r, 'z-index')).toBe('1');
    expect(value(r, 'background')).toBe('var(--color-background-transparent)');
    expect(value(r, 'overflow-x')).toBe('hidden');
  });

  it('.loader-overlay is centred text, 2em, primary colour', () => {
    const r = rules('.loader-overlay');
    expect(value(r, 'justify-content')).toBe('center');
    expect(value(r, 'align-items')).toBe('center');
    expect(value(r, 'flex-grow')).toBe('1');
    expect(value(r, 'font-size')).toBe('2em');
    expect(value(r, 'color')).toBe('var(--color-primary)');
    expect(value(r, 'text-align')).toBe('center');
  });

  it('.loader-spinner-box (SpinnerWrapper) starts at calc(50vh - 150px)', () => {
    expect(value(rules('.loader-spinner-box'), 'margin-top')).toBe('calc(50vh - 150px)');
  });

  it('.loader-spinner (StyledSpinner) is 50px x 50px with 10px below', () => {
    const r = rules('.loader-spinner');
    expect(value(r, 'height')).toBe('50px');
    expect(value(r, 'width')).toBe('50px');
    expect(value(r, 'margin-bottom')).toBe('10px');
  });

  it('.loader-text (the "DPDS" wordmark) is bold and in the primary colour, and does not rescale the 2em', () => {
    const r = rules('.loader-text');
    expect(value(r, 'font-weight') ?? '').toMatch(/^(bold|[6-9]00)$/);
    expect(value(r, 'color')).toBe('var(--color-primary)');
    expect(value(r, 'font-size') ?? 'inherit').toMatch(/^(inherit|1em|100%)$/);
  });
});

describe('--color-background-transparent (spec 54 L2)', () => {
  it('is rgba(255, 255, 255, 0.8) in the light theme (:root)', () => {
    expect(value(rules(':root'), '--color-background-transparent')).toBe('rgba(255, 255, 255, 0.8)');
  });

  it('is rgba(30, 30, 30, 0.8) in the dark theme ([data-theme="dark"])', () => {
    expect(value(rules('[data-theme="dark"]'), '--color-background-transparent')).toBe('rgba(30, 30, 30, 0.8)');
  });
});
