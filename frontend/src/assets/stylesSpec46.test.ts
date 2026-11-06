// The top row's pills are styled inline (pinned in pages/dashboardTopRow.test.tsx), so the stylesheet has no pill rules; the colour
// variables they use must stay.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

describe('index.css (spec 46)', () => {
  it('has no .status-pill and no .status-pill-tx rule any more', () => {
    expect(css).not.toMatch(/status-pill/);
  });

  it.each(['--color-secondary', '--color-primary', '--color-okay', '--color-yellow'])(
    'still defines %s, which the pills reference',
    (name) => {
      expect(css).toMatch(new RegExp(`${name}\\s*:\\s*[^;]+;`));
    },
  );
});
