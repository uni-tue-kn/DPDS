// Security guard: the controller serves the frontend under a strict CSP (default-src 'self', no inline code), so
// index.html must load only same-origin files and contain no inline script, style or event handler.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const html = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '..', 'index.html'), 'utf8');

describe('index.html works under the controller CSP', () => {
  it('has no inline <script> body, inline <style> element, style= or on*= attributes', () => {
    const scripts = Array.from(html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi));
    expect(scripts.length).toBeGreaterThan(0);
    for (const [, attrs, body] of scripts) {
      expect(attrs).toMatch(/\bsrc=/);
      expect(body.trim()).toBe('');
    }
    expect(html).not.toMatch(/<style\b/i);
    expect(html).not.toMatch(/\sstyle\s*=/i);
    expect(html).not.toMatch(/\son[a-z]+\s*=/i);
  });

  it('loads nothing from another origin', () => {
    expect(html).not.toMatch(/(src|href)\s*=\s*["']?(https?:)?\/\//i);
  });
});
