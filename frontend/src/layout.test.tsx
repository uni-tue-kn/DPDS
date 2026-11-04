import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import { api } from './test/mockApi';
import * as fx from './test/fixtures';
import { renderApp, tick } from './test/helpers';
import { setPrefersDark } from './test/matchMedia';

const NAV = ['Dashboard', 'Links', 'Interfaces', 'Host hints', 'Settings'];
const PAGES: [string, string, string][] = [
  ['Dashboard', '/', 'Dashboard'],
  ['Links', '/links', 'Links'],
  ['Interfaces', '/interfaces', 'Interfaces'],
  ['Host hints', '/hints', 'Host hints'],
  ['Settings', '/settings', 'Settings'],
];

describe('sidebar navigation', () => {
  it('has a main navigation with the five pages in order', () => {
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    const names = within(nav)
      .getAllByRole('link')
      .map((a) => a.getAttribute('aria-label') ?? a.textContent?.trim());
    expect(names.slice(0, 5)).toEqual(NAV);
  });

  it('has no Presets item: the navigation holds exactly the five pages (spec27)', () => {
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    const names = within(nav)
      .getAllByRole('link')
      .map((a) => a.getAttribute('aria-label') ?? a.textContent?.trim());
    expect(names).toEqual(NAV);
    expect(screen.queryByRole('link', { name: /preset/i })).not.toBeInTheDocument();
    expect(document.querySelector('a[href="/presets"]')).toBeNull();
  });

  it.each(['/presets', '/presets/new', '/presets/1'])(
    'the route %s is gone: it falls through to the dashboard, no Presets page',
    async (path) => {
      renderApp(path);
      expect(await screen.findByRole('heading', { level: 1, name: 'Dashboard' })).toBeInTheDocument();
      expect(screen.queryByRole('heading', { name: /preset/i })).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: /preset/i })).not.toBeInTheDocument();
    },
  );

  it.each(PAGES)('link "%s" opens %s with the matching heading', async (name, _path, heading) => {
    const user = userEvent.setup();
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    await user.click(within(nav).getByRole('link', { name }));
    expect(await screen.findByRole('heading', { level: 1, name: heading })).toBeInTheDocument();
  });

  it.each(PAGES)('marks the current page link (%s) with aria-current', (name, path) => {
    renderApp(path);
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(within(nav).getByRole('link', { name })).toHaveAttribute('aria-current', 'page');
    for (const other of NAV.filter((n) => n !== name)) {
      expect(within(nav).getByRole('link', { name: other })).not.toHaveAttribute('aria-current', 'page');
    }
  });

  it.each(PAGES)('direct route %s renders heading %s', async (_name, path, heading) => {
    renderApp(path);
    expect(await screen.findByRole('heading', { level: 1, name: heading })).toBeInTheDocument();
  });
});

describe('dark mode', () => {
  const toggle = () => screen.getByRole('button', { name: 'Toggle dark mode' });

  it('defaults to light when the OS does not prefer dark', () => {
    setPrefersDark(false);
    renderApp('/');
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
    expect(toggle()).toHaveAttribute('aria-pressed', 'false');
  });

  it('defaults to dark when the OS prefers dark', () => {
    setPrefersDark(true);
    renderApp('/');
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
    expect(toggle()).toHaveAttribute('aria-pressed', 'true');
  });

  it('the stored choice wins over prefers-color-scheme', () => {
    setPrefersDark(true);
    localStorage.setItem('theme', 'light');
    renderApp('/');
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
  });

  it('the stored dark choice wins over a light OS', () => {
    setPrefersDark(false);
    localStorage.setItem('theme', 'dark');
    renderApp('/');
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
  });

  it('toggling flips data-theme and persists the choice in localStorage', async () => {
    const user = userEvent.setup();
    setPrefersDark(false);
    renderApp('/');
    await user.click(toggle());
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
    expect(localStorage.getItem('theme')).toBe('dark');
    expect(toggle()).toHaveAttribute('aria-pressed', 'true');
    await user.click(toggle());
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
    expect(localStorage.getItem('theme')).toBe('light');
  });

  it('does not write localStorage before the user toggles (the OS preference stays authoritative)', () => {
    setPrefersDark(true);
    renderApp('/');
    expect(localStorage.getItem('theme')).toBeNull();
  });

  it('the choice survives a remount (reload)', async () => {
    const user = userEvent.setup();
    setPrefersDark(false);
    const first = renderApp('/');
    await user.click(toggle());
    first.unmount();
    document.documentElement.removeAttribute('data-theme');
    renderApp('/');
    await waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'dark'));
  });

  it('the toggle is available on every page (sidebar)', () => {
    renderApp('/settings');
    expect(toggle()).toBeInTheDocument();
  });
});

// v16: P4TG-style API documentation link (a book icon at the bottom of the sidebar, opens the controller's Swagger UI).
describe('sidebar API documentation link (v16)', () => {
  const docs = () => screen.getByRole('link', { name: 'API documentation' });

  it('is a link named "API documentation" to /api/docs that opens a new tab safely', () => {
    renderApp('/');
    const a = docs();
    expect(a.tagName).toBe('A');
    expect(a).toHaveAttribute('href', '/api/docs');
    expect(a).toHaveAttribute('target', '_blank');
    const rel = (a.getAttribute('rel') ?? '').split(/\s+/);
    expect(rel).toContain('noopener');
    expect(rel).toContain('noreferrer');
    expect(a).toHaveAttribute('title', 'API documentation');
  });

  it('shows the bootstrap-icons book (bi-book) inside the link, hidden from assistive technology', () => {
    renderApp('/');
    const icon = docs().querySelector('i.bi.bi-book');
    expect(icon).not.toBeNull();
    expect(icon).toHaveAttribute('aria-hidden', 'true');
  });

  it('sits in the sidebar after the navigation entries, outside the page navigation', () => {
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(docs().closest('.sidebar')).not.toBeNull();
    expect(nav.closest('.sidebar')).toBe(docs().closest('.sidebar'));
    expect(nav).not.toContainElement(docs());
    expect(nav.compareDocumentPosition(docs()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // the page links keep their five entries in order (the docs link is not one of them)
    const names = within(nav)
      .getAllByRole('link')
      .map((a) => a.getAttribute('aria-label') ?? a.textContent?.trim());
    expect(names.slice(0, 5)).toEqual(NAV);
    expect(names).not.toContain('API documentation');
  });

  it('is the last link of the sidebar (after every page link)', () => {
    renderApp('/');
    const sidebar = docs().closest('.sidebar') as HTMLElement;
    const links = within(sidebar).getAllByRole('link');
    expect(links[links.length - 1]).toBe(docs());
  });

  it('is a plain anchor, not a router link: never marked as the current page', () => {
    renderApp('/');
    expect(docs()).not.toHaveAttribute('aria-current');
  });

  it.each(PAGES)('is available on every page (%s)', (_name, path) => {
    renderApp(path);
    expect(docs()).toHaveAttribute('href', '/api/docs');
  });

  it('stays next to the dark-mode toggle and the version line (nothing replaced)', () => {
    renderApp('/');
    expect(screen.getByRole('button', { name: 'Toggle dark mode' })).toBeInTheDocument();
    expect(docs().closest('.sidebar')).toContainElement(screen.getByRole('button', { name: 'Toggle dark mode' }));
  });
});

describe('sidebar server status (F3)', () => {
  it('shows the version once /api/online answers', async () => {
    renderApp('/hints');
    expect(await screen.findByText(/v1\.0\.0/)).toBeInTheDocument();
  });

  it('shows "Offline" while /api/online fails and recovers by itself (re-poll every <= 5 s)', async () => {
    vi.useFakeTimers();
    api.getOnline
      .mockRejectedValueOnce(new Error('Failed to fetch'))
      .mockResolvedValue(fx.online({ version: '0.9.9' }));
    renderApp('/hints');
    await tick(0);
    expect(screen.getByText('Offline')).toBeInTheDocument();
    await tick(5500);
    expect(screen.getByText(/v0\.9\.9/)).toBeInTheDocument();
    expect(screen.queryByText('Offline')).not.toBeInTheDocument();
  });

  it('keeps polling /api/online while it answers (at least every 5 s)', async () => {
    vi.useFakeTimers();
    renderApp('/hints');
    await tick(0);
    const first = api.getOnline.mock.calls.length;
    await tick(12000);
    expect(api.getOnline.mock.calls.length).toBeGreaterThanOrEqual(first + 2);
  });

  it('never has two /api/online requests in flight', async () => {
    vi.useFakeTimers();
    const probe = { active: 0, max: 0 };
    api.getOnline.mockImplementation(async () => {
      probe.active += 1;
      probe.max = Math.max(probe.max, probe.active);
      await new Promise((r) => setTimeout(r, 12000));
      probe.active -= 1;
      return fx.online();
    });
    renderApp('/hints');
    for (let i = 0; i < 40; i += 1) await tick(1000);
    expect(probe.max).toBe(1);
    expect(api.getOnline.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('stops polling /api/online after unmount', async () => {
    vi.useFakeTimers();
    const view = renderApp('/hints');
    await tick(6000);
    view.unmount();
    const calls = api.getOnline.mock.calls.length;
    await tick(30000);
    expect(api.getOnline.mock.calls.length).toBe(calls);
  });
});
