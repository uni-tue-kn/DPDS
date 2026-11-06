import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import logo from './assets/logo.png';
import logoDark from './assets/logo-dark.png';
import * as fx from './test/fixtures';
import { renderApp, seedLinks } from './test/helpers';
import { setPrefersDark } from './test/matchMedia';
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

// Pages show no visible heading: the h1 keeps the page name as its accessible name, but contains only the visually hidden name.
describe('page title', () => {
  it.each(PAGES)('on %s the only h1 is named "%s" and holds only the visually hidden name', async (path, name) => {
    renderApp(path);
    const h1 = await screen.findByRole('heading', { level: 1, name });
    expect(screen.getAllByRole('heading', { level: 1 })).toEqual([h1]);
    expect(h1).toHaveAccessibleName(name);

    const hidden = must(h1.querySelector('.visually-hidden'), 'the visually hidden page name') as HTMLElement;
    expect(hidden.textContent?.trim()).toBe(name);
    expect(h1.querySelector('img')).toBeNull();

    const visible = Array.from(h1.childNodes)
      .filter((n) => n !== hidden)
      .map((n) => n.textContent ?? '')
      .join('');
    expect(visible.trim()).toBe('');
  });
});

// The DPDS logo sits at the top of the sidebar, as a plain image: not a link, no GitHub link in the sidebar (the footer keeps it).
describe('sidebar brand', () => {
  it.each(PAGES)('on %s shows the unlinked "DPDS logo" in .sidebar-brand', async (path, name) => {
    renderApp(path);
    await screen.findByRole('heading', { level: 1, name });
    const sidebar = must(document.querySelector('.sidebar'), 'the sidebar') as HTMLElement;
    const brand = must(sidebar.querySelector('.sidebar-brand'), 'the sidebar brand') as HTMLElement;
    const logo = must(brand.querySelector('img'), 'the logo in the brand') as HTMLImageElement;
    expect(logo.getAttribute('src')).toMatch(/logo/);
    expect(logo.getAttribute('alt')).toBe('DPDS logo');
    expect(screen.getByRole('img', { name: 'DPDS logo' })).toBe(logo);
    expect(sidebar.querySelectorAll('img')).toHaveLength(1);
    expect(logo.closest('a')).toBeNull();
    expect(brand.querySelector('a')).toBeNull();
    expect(sidebar.querySelector('a[href="https://github.com/uni-tue-kn/DPDS"]')).toBeNull();
  });

  it('is the first thing in the sidebar, before the navigation', async () => {
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    const sidebar = must(document.querySelector('.sidebar'), 'the sidebar') as HTMLElement;
    const brand = must(sidebar.querySelector('.sidebar-brand'), 'the sidebar brand');
    const nav = must(sidebar.querySelector('nav'), 'the sidebar nav');
    expect(brand.compareDocumentPosition(nav) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

// The brand image follows the theme: logo.png in light, logo-dark.png in dark, switching with the toggle; never a second <img>.
describe('sidebar brand logo follows the theme', () => {
  const brandImgs = () => {
    const brand = must(document.querySelector('.sidebar .sidebar-brand'), 'the sidebar brand') as HTMLElement;
    return Array.from(brand.querySelectorAll('img'));
  };
  const brandSrc = () => {
    const imgs = brandImgs();
    expect(imgs).toHaveLength(1);
    return must(imgs[0]).getAttribute('src');
  };
  const toggle = () => screen.getByRole('button', { name: 'Toggle dark mode' });

  it('uses two distinct assets', () => {
    expect(logo).toBeTruthy();
    expect(logoDark).toBeTruthy();
    expect(logoDark).not.toBe(logo);
  });

  it('shows logo.png in the light theme', async () => {
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    expect(brandSrc()).toBe(logo);
  });

  it('shows logo-dark.png when the stored theme is dark', async () => {
    localStorage.setItem('theme', 'dark');
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    expect(brandSrc()).toBe(logoDark);
  });

  it('shows logo-dark.png when the system prefers dark and nothing is stored', async () => {
    setPrefersDark(true);
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    expect(brandSrc()).toBe(logoDark);
  });

  it('switches immediately with the toggle, light to dark and back, keeping one img', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    expect(brandSrc()).toBe(logo);

    await user.click(toggle());
    expect(brandSrc()).toBe(logoDark);

    await user.click(toggle());
    expect(brandSrc()).toBe(logo);
  });

  it('switches from an initial dark theme to light and back', async () => {
    const user = userEvent.setup();
    localStorage.setItem('theme', 'dark');
    renderApp('/');
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    expect(brandSrc()).toBe(logoDark);

    await user.click(toggle());
    expect(brandSrc()).toBe(logo);

    await user.click(toggle());
    expect(brandSrc()).toBe(logoDark);
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
