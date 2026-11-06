import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import * as fx from './test/fixtures';
import { renderApp, seedLinks } from './test/helpers';
import { must } from './test/must';

const PAGES: [string, string][] = [
  ['/', 'Dashboard'],
  ['/interfaces', 'Interfaces'],
  ['/hints', 'Host hints'],
  ['/settings', 'Settings'],
  ['/settings/new', 'New link'],
  ['/settings/1', 'Edit link'],
];

beforeEach(() => {
  seedLinks([fx.link()]);
});

// The DPDS logo is the visible title of every page: the h1 keeps the page name as its accessible name, visually hidden, and
// shows the decorative logo image (alt "") instead.
describe('page title logo', () => {
  it.each(PAGES)(
    'on %s the only h1 is named "%s", hides the name and shows the decorative logo',
    async (path, name) => {
      renderApp(path);
      const h1 = await screen.findByRole('heading', { level: 1, name });
      expect(screen.getAllByRole('heading', { level: 1 })).toEqual([h1]);
      expect(h1).toHaveAccessibleName(name);

      const hidden = must(h1.querySelector('.visually-hidden'), 'the visually hidden page name') as HTMLElement;
      expect(hidden.textContent?.trim()).toBe(name);

      const logo = must(h1.querySelector('img'), 'the logo inside the h1') as HTMLImageElement;
      expect(logo.getAttribute('src')).toMatch(/logo/);
      expect(logo.getAttribute('alt')).toBe('');
      expect(h1.querySelectorAll('img')).toHaveLength(1);

      const visible = Array.from(h1.childNodes)
        .filter((n) => n !== hidden)
        .map((n) => n.textContent ?? '')
        .join('');
      expect(visible.trim()).toBe('');
    },
  );
});

// The sidebar no longer carries the logo or the GitHub link (the footer keeps it).
describe('sidebar without brand', () => {
  it.each(PAGES)('on %s has no brand, no image and no GitHub link', async (path, name) => {
    renderApp(path);
    await screen.findByRole('heading', { level: 1, name });
    const sidebar = must(document.querySelector('.sidebar'), 'the sidebar') as HTMLElement;
    expect(document.querySelector('.sidebar-brand')).toBeNull();
    expect(sidebar.querySelector('img')).toBeNull();
    expect(sidebar.querySelector('a[href="https://github.com/uni-tue-kn/DPDS"]')).toBeNull();
    expect(screen.queryByRole('img', { name: 'DPDS logo' })).not.toBeInTheDocument();
  });
});

describe('logo asset', () => {
  it('ships a small copy of the README logo, not the 141 kB original', () => {
    const size = readFileSync(resolve(__dirname, 'assets/logo.png')).length;
    expect(size).toBeGreaterThan(0);
    expect(size).toBeLessThan(40_000);
  });
});

// A footer at the end of the main content, on every page: "DPDS @ " and the GitHub icon, linking to the GitHub page.
describe('page footer', () => {
  it.each(['/', '/interfaces', '/hints', '/settings'])(
    'on %s reads "DPDS @" with the GitHub icon and links to the repository',
    (path) => {
      renderApp(path);
      const footer = must(document.querySelector('main.main-content > footer.app-footer'), 'the footer') as HTMLElement;
      const link = must(footer.querySelector('a'), 'the footer link') as HTMLAnchorElement;
      expect(link.getAttribute('href')).toBe('https://github.com/uni-tue-kn/DPDS');
      expect(link).toHaveAttribute('target', '_blank');
      expect((link.getAttribute('rel') ?? '').split(/\s+/)).toEqual(expect.arrayContaining(['noopener', 'noreferrer']));
      expect(link.textContent?.replace(/\s+/g, ' ').trim()).toBe('DPDS @');
      expect(link.querySelector('i.bi.bi-github')).not.toBeNull();
      expect(link).toHaveAccessibleName(/DPDS @ GitHub/);
      expect(footer).toBe(must(footer.parentElement).lastElementChild);
    },
  );
});
