import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// From the md breakpoint on, a vertical line separates the donut columns: the class is `donut-divider` (on the left column, see
// dashboardSpec52.test.tsx); index.css has the rule `.donut-divider { border-right: 1px solid var(--color-border); }` inside
// `@media (min-width: 768px)` (Bootstrap's md) and no rule for the class outside such a block declares a border, so below md there
// is none.

const css = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), 'assets/index.css'), 'utf8').replace(
  /\/\*[\s\S]*?\*\//g,
  '',
);

interface Block {
  prelude: string;
  body: string;
}

/** The blocks `prelude { body }` at the top level of `text` (braces nest). */
function blocks(text: string): Block[] {
  const out: Block[] = [];
  let depth = 0;
  let start = 0;
  let open = 0;
  let prelude = '';
  for (let i = 0; i < text.length; i++) {
    if (text[i] === '{') {
      if (depth === 0) {
        prelude = text.slice(start, i).trim();
        open = i + 1;
      }
      depth++;
    } else if (text[i] === '}') {
      depth--;
      if (depth === 0) {
        out.push({ prelude, body: text.slice(open, i) });
        start = i + 1;
      }
    }
  }
  return out;
}

const squash = (s: string) => s.replace(/\s+/g, ' ').trim().toLowerCase();
const mentions = (b: Block) =>
  b.prelude.split(',').some((sel) => /(^|\s)\.donut-divider(\s|$|[:.[>+~])/.test(sel.trim()));

const top = blocks(css);
const topRules = top.filter((b) => !b.prelude.startsWith('@') && mentions(b));
const media = top.filter((b) => b.prelude.startsWith('@media'));
const mediaRules = media.flatMap((m) =>
  blocks(m.body)
    .filter(mentions)
    .map((r) => ({ query: squash(m.prelude), rule: r })),
);

describe('Spec 52 R1: the .donut-divider rule of index.css', () => {
  it('exists exactly once, inside an @media (min-width: 768px) block', () => {
    expect(mediaRules).toHaveLength(1);
    expect(mediaRules[0].query.replace(/\s/g, '')).toBe('@media(min-width:768px)');
  });

  it('draws a right border of 1px solid var(--color-border) and nothing else', () => {
    const decls = squash(mediaRules[0].rule.body)
      .split(';')
      .map((d) => d.trim())
      .filter(Boolean);
    expect(decls).toEqual(['border-right: 1px solid var(--color-border)']);
  });

  it('has no border outside that media query (the stacked columns have no line below md)', () => {
    for (const r of topRules) expect(squash(r.body)).not.toMatch(/border/);
    for (const m of media.filter((m) => !squash(m.prelude).replace(/\s/g, '').includes('(min-width:768px)'))) {
      for (const r of blocks(m.body).filter(mentions)) expect(squash(r.body), m.prelude).not.toMatch(/border/);
    }
  });
});
