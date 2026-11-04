// Spec 33b (frontend item 1): the commands tooltip of the host hints must show long commands in full. jsdom renders no CSS,
// so the looks are pinned in the stylesheet itself, like stylesSpec28.test.ts does; the DOM side (the tooltip's root element
// carries the class `commands-tip`) is pinned in pages/hostCommands.test.tsx.
// Background: Bootstrap caps `.tooltip-inner` with `max-width: var(--bs-tooltip-max-width)` (200px by default), so a wide
// tooltip needs that variable on `.commands-tip` (as `.info-tip-text` does) or a `max-width` on its `.tooltip-inner`. A
// `max-width` on the outer `.commands-tip` alone changes nothing, so it does not count.
// CHOICES of the tester: "wide" means at least 30rem (480px); rem and px are the only accepted units ("none" would let the
// tooltip outgrow the page); the word-breaking rule may be word-break: break-all | break-word or overflow-wrap: anywhere |
// break-word; the `pre` must not scroll sideways (overflow-x: auto | scroll is refused).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8');
const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

/** All declarations of every rule whose selector list includes one of `selectors` (whitespace-insensitive). */
function rules(...selectors: string[]): string {
  const wanted = selectors.map(norm);
  const re = /([^{}]+)\{([^{}]*)\}/g;
  const out: string[] = [];
  for (let m = re.exec(css); m !== null; m = re.exec(css)) {
    if (
      m[1]
        .split(',')
        .map(norm)
        .some((s) => wanted.includes(s))
    )
      out.push(m[2]);
  }
  return out.join('\n');
}

/** The last value given for `property` in `declarations`, or null. */
function lastValue(declarations: string, property: string): string | null {
  const re = new RegExp(`(?:^|[;\\s{])${property.replace(/[-]/g, '\\-')}\\s*:\\s*([^;}]+)`, 'g');
  let last: string | null = null;
  for (let m = re.exec(declarations); m !== null; m = re.exec(declarations)) last = m[1].trim();
  return last;
}

/** A length in rem, or null when it is not a plain rem/px number. */
function inRem(value: string | null): number | null {
  const m = /^(\d+(?:\.\d+)?)(rem|px)$/.exec(value ?? '');
  if (!m) return null;
  return m[2] === 'rem' ? Number(m[1]) : Number(m[1]) / 16;
}

describe('commands tooltip looks (spec 33b)', () => {
  it('has a .commands-tip rule', () => {
    expect(rules('.commands-tip', '.tooltip.commands-tip'), '.commands-tip is styled').not.toBe('');
  });

  it('is at least 30rem wide: the Bootstrap variable on .commands-tip or max-width on its .tooltip-inner', () => {
    const inner = lastValue(
      rules('.commands-tip .tooltip-inner', '.tooltip.commands-tip .tooltip-inner', '.commands-tip > .tooltip-inner'),
      'max-width',
    );
    const variable = lastValue(rules('.commands-tip', '.tooltip.commands-tip'), '--bs-tooltip-max-width');
    const effective = inner ?? variable;
    expect(effective, "a max-width that Bootstrap's .tooltip-inner actually uses").not.toBeNull();
    const rem = inRem(effective);
    expect(rem, `"${effective}" is a rem or px length`).not.toBeNull();
    expect(rem).toBeGreaterThanOrEqual(30);
  });

  const PRE = ['.commands-tip pre', '.tooltip.commands-tip pre', '.commands-tip .tooltip-inner pre'] as const;

  it('its pre wraps long lines: white-space: pre-wrap', () => {
    expect(lastValue(rules(...PRE), 'white-space')).toBe('pre-wrap');
  });

  it('its pre breaks inside long words (a path or an argument without spaces cannot push it wider)', () => {
    const decl = rules(...PRE);
    const wordBreak = lastValue(decl, 'word-break');
    const overflowWrap = lastValue(decl, 'overflow-wrap') ?? lastValue(decl, 'word-wrap');
    const breaks =
      (wordBreak !== null && /^(break-all|break-word)$/.test(wordBreak)) ||
      (overflowWrap !== null && /^(anywhere|break-word)$/.test(overflowWrap));
    expect(breaks, `word-break "${wordBreak}", overflow-wrap "${overflowWrap}"`).toBe(true);
  });

  it('its pre has no horizontal scrollbar', () => {
    const decl = rules(...PRE);
    for (const p of ['overflow', 'overflow-x'])
      expect(lastValue(decl, p) ?? 'visible').not.toMatch(/\b(auto|scroll)\b/);
  });
});
