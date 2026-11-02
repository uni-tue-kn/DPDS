// The P4TG palette must live in CSS variables (jsdom does not process CSS, so the file is read directly).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const css = readFileSync(resolve(__dirname, 'index.css'), 'utf8');

function block(selector: RegExp): string {
  const m = selector.exec(css);
  if (!m) throw new Error(`no block for ${selector}`);
  const start = css.indexOf('{', m.index);
  let depth = 0;
  for (let i = start; i < css.length; i += 1) {
    if (css[i] === '{') depth += 1;
    if (css[i] === '}') {
      depth -= 1;
      if (depth === 0) return css.slice(start + 1, i);
    }
  }
  throw new Error('unterminated block');
}

describe('index.css theme variables', () => {
  it(':root defines the P4TG colours', () => {
    const root = block(/:root\s*\{/);
    expect(root).toMatch(/--color-primary:\s*#e74c3c/i);
    expect(root).toMatch(/--color-secondary:\s*#2c3e50/i);
    expect(root).toMatch(/--color-okay:\s*#27ae60/i);
    expect(root).toMatch(/--color-yellow:\s*#ffc107/i);
    expect(root).toMatch(/--color-background:/);
    expect(root).toMatch(/--color-sidebar-background:/);
    expect(root).toMatch(/--color-text:/);
  });

  it('[data-theme="dark"] overrides the background, sidebar and secondary colours', () => {
    const dark = block(/\[data-theme=["']?dark["']?\]\s*\{/);
    expect(dark).toMatch(/--color-background:/);
    expect(dark).toMatch(/--color-sidebar-background:/);
    expect(dark).toMatch(/--color-secondary:/);
    expect(dark).toMatch(/--color-text:/);
  });

  it('uses the variables in rules (not only defines them)', () => {
    expect(css).toMatch(/var\(--color-primary\)/);
    expect(css).toMatch(/var\(--color-sidebar-background\)/);
  });
});
