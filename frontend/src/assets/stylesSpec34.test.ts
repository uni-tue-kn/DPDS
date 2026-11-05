// Spec 34 (frontend): the Recheck button of the Host hints page spins its icon while a check runs. The DOM side (the icon
// `i.bi-arrow-clockwise` carries the class `icon-spin` exactly while the check runs) is pinned in pages/hintsRecheck.test.tsx;
// jsdom renders no CSS, so the animation is pinned in the stylesheet itself, like stylesSpec33b.test.ts does.
// CHOICES of the tester:
// - the class is called `icon-spin`;
// - the icon is an inline `<i>`, and a transform does not apply to an inline box, so the rule must make it `inline-block`
//   (or block / flex / inline-flex);
// - the animation is a rotation by a full turn (`rotate(360deg)` or `rotate(1turn)`) that repeats for ever (`infinite`);
// - it is off under `prefers-reduced-motion: reduce`, in one of two ways: a `@media (prefers-reduced-motion: reduce)` rule for
//   `.icon-spin` that sets `animation: none` (or `animation-name: none`), or the animation lives only inside
//   `@media (prefers-reduced-motion: no-preference)`.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { must } from '../test/must';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

interface Block {
  prelude: string;
  body: string;
}

/** The blocks at the top level of `text` (a rule or an at-rule each), braces matched. */
function blocks(text: string): Block[] {
  const out: Block[] = [];
  let depth = 0;
  let start = 0;
  let bodyStart = 0;
  let prelude = '';
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '{') {
      if (depth === 0) {
        prelude = norm(text.slice(start, i));
        bodyStart = i + 1;
      }
      depth++;
    } else if (c === '}') {
      depth--;
      if (depth === 0) {
        out.push({ prelude, body: text.slice(bodyStart, i) });
        start = i + 1;
      }
    }
  }
  return out;
}

const SELECTORS = ['.icon-spin', 'i.icon-spin', '.bi.icon-spin', '.bi-arrow-clockwise.icon-spin'];

/** All declarations of the plain rules of `text` whose selector list includes `.icon-spin` (not of at-rules). */
function spinRules(text: string): string {
  return blocks(text)
    .filter((b) => !b.prelude.startsWith('@') && b.prelude.split(',').some((s) => SELECTORS.includes(norm(s))))
    .map((b) => b.body)
    .join('\n');
}

/** The last value given for `property` in `declarations`, or null. */
function lastValue(declarations: string, property: string): string | null {
  const re = new RegExp(`(?:^|[;\\s{])${property.replace(/-/g, '\\-')}\\s*:\\s*([^;}]+)`, 'g');
  let last: string | null = null;
  for (let m = re.exec(declarations); m !== null; m = re.exec(declarations)) last = m[1].trim();
  return last;
}

const topLevel = blocks(css);
const media = (condition: RegExp) =>
  topLevel
    .filter((b) => b.prelude.startsWith('@media') && condition.test(b.prelude))
    .map((b) => b.body)
    .join('\n');
const reduce = media(/prefers-reduced-motion\s*:\s*reduce/);
const noPreference = media(/prefers-reduced-motion\s*:\s*no-preference/);

/** The animation declarations of `text` for `.icon-spin`: the shorthand plus a separate `animation-name`, when given. */
function animationOf(text: string): { shorthand: string | null; name: string | null } {
  const decl = spinRules(text);
  return { shorthand: lastValue(decl, 'animation'), name: lastValue(decl, 'animation-name') };
}
const isNone = (a: { shorthand: string | null; name: string | null }) =>
  (a.name ?? a.shorthand ?? '').split(/\s+/).includes('none');
const hasAnimation = (a: { shorthand: string | null; name: string | null }) =>
  (a.shorthand !== null || a.name !== null) && !isNone(a);

describe('the spinning Recheck icon (spec 34)', () => {
  const plain = topLevel.filter((b) => !b.prelude.startsWith('@media'));
  const base = plain.map((b) => `${b.prelude}{${b.body}}`).join('\n');

  it('has a .icon-spin rule', () => {
    expect(spinRules(base) + spinRules(noPreference), '.icon-spin is styled').not.toBe('');
  });

  it('is displayed inline-block (a transform does not apply to an inline <i>)', () => {
    const display = lastValue(`${spinRules(base)}\n${spinRules(noPreference)}`, 'display');
    expect(display, 'display of .icon-spin').toMatch(/^(inline-block|block|flex|inline-flex)$/);
  });

  // The rule that animates, wherever it sits (outside any media query, or in the no-preference one).
  const animation = () => {
    const outside = animationOf(base);
    return hasAnimation(outside) ? outside : animationOf(noPreference);
  };

  it('runs a keyframes animation that is defined in the stylesheet and repeats for ever', () => {
    const a = animation();
    expect(hasAnimation(a), 'an animation on .icon-spin').toBe(true);
    const decl = `${spinRules(base)}\n${spinRules(noPreference)}`;
    const text = `${a.shorthand ?? ''} ${a.name ?? ''}`;
    const defined = [...css.matchAll(/@keyframes\s+([\w-]+)/g)].map((m) => m[1]);
    const used = defined.filter((name) => text.split(/\s+/).includes(name));
    expect(used, `a defined @keyframes name in "${text.trim()}"`).toHaveLength(1);
    const iterations = lastValue(decl, 'animation-iteration-count') ?? '';
    expect(`${a.shorthand ?? ''} ${iterations}`, 'repeats for ever').toMatch(/\binfinite\b/);
  });

  it('the keyframes turn the icon a full turn', () => {
    const a = animation();
    const text = `${a.shorthand ?? ''} ${a.name ?? ''}`.split(/\s+/);
    const frames = topLevel.filter((b) => {
      const m = /^@keyframes\s+([\w-]+)$/.exec(b.prelude);
      return m !== null && text.includes(m[1]);
    });
    expect(frames).toHaveLength(1);
    expect(must(frames[0]).body).toMatch(/rotate\(\s*(360deg|1turn)\s*\)/);
  });

  it('is off under prefers-reduced-motion: reduce', () => {
    const outside = animationOf(base);
    if (hasAnimation(outside)) {
      // variant 1: animated by default, a reduce rule that comes after turns it off
      expect(isNone(animationOf(reduce)), 'a reduce rule with animation: none for .icon-spin').toBe(true);
    } else {
      // variant 2: the animation only exists for no-preference
      expect(hasAnimation(animationOf(noPreference)), 'animated only for no-preference').toBe(true);
      expect(hasAnimation(animationOf(reduce)), 'nothing animates under reduce').toBe(false);
    }
  });
});
