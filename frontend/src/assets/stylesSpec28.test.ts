// Spec 28 (P4TG-like sidebar) and spec 30 frontend (power-of-two histogram hints): the looks that jsdom cannot render are
// pinned in the stylesheet itself, like styles.test.ts does. There is no browser here, so these only prove the rules exist.
// CHOICES of the tester: the sidebar is at most 90px wide (P4TG's icon column), the active link is a solid
// var(--color-primary) block with white text, the footer is centred, and the P4TG hint classes carry P4TG's colours.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8');

/** The declarations of the first rule whose selector (the text before `{`) matches exactly `selector` (whitespace-insensitive). */
function rule(selector: string): string {
  const norm = (s: string) => s.replace(/\s+/g, ' ').trim();
  const re = /([^{}]+)\{([^{}]*)\}/g;
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    const selectors = m[1].split(',').map(norm);
    if (selectors.includes(norm(selector))) return m[2];
  }
  throw new Error(`no rule for ${selector}`);
}
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

describe('sidebar looks (spec 28)', () => {
  it('the top-level .sidebar rule makes it a narrow icon column (<= 90px)', () => {
    const w = /(?:^|[;\s])width:\s*(\d+(?:\.\d+)?)px/.exec(rule('.sidebar'));
    expect(w, '.sidebar has a pixel width').not.toBeNull();
    expect(Number(w?.[1])).toBeLessThanOrEqual(90);
  });

  it('the active link is a solid red block with a white icon', () => {
    const active = rules('.sidebar .nav-link.active');
    expect(active).toMatch(/background(?:-color)?:\s*var\(--color-primary\)/);
    expect(active).toMatch(/color:\s*(?:#fff(?:fff)?\b|white|var\(--color-sidebar-selected-icon\))/i);
    expect(active).not.toMatch(/background(?:-color)?:\s*var\(--color-card-background\)/);
  });

  it('the footer content is centred', () => {
    const footer = rules('.sidebar-footer');
    expect(footer).toMatch(/text-align:\s*center|align-items:\s*center/);
  });

  it('nothing hides the navigation icons (display: none must not hit .nav-link i)', () => {
    expect(css).not.toMatch(/\.nav-link\s+i\s*\{[^}]*display:\s*none/);
  });
});

describe('histogram hints looks (spec 30, P4TG .histogram-optimization-*)', () => {
  it('has the hint block and a text button class for "Use suggestion"', () => {
    expect(rules('.histogram-optimization-hint')).not.toBe('');
    const action = rules('.histogram-optimization-action');
    expect(action).not.toBe('');
    expect(action).toMatch(/background:\s*(?:none|transparent)|border:\s*(?:0|none)/);
  });

  it('the aligned icon is green (--color-okay), the suggestion icon amber (#b7791f, dark #e0aa45)', () => {
    expect(rules('.histogram-optimization-icon-aligned')).toMatch(/color:\s*var\(--color-okay\)/);
    expect(rules('.histogram-optimization-icon-suggestion')).toMatch(/color:\s*#b7791f/i);
    expect(css).toMatch(/\[data-theme=["']?dark["']?\][^{]*\.histogram-optimization-icon-suggestion\s*\{[^}]*#e0aa45/i);
  });
});
