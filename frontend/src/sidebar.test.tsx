import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderApp } from './test/helpers';
import { setPrefersDark } from './test/matchMedia';
import { must } from './test/must';

// Spec 28 item 3 (user 2026-11-04): the P4TG icon-only sidebar. CHOICES of the tester where the spec is silent:
//   - "icon-only": a navigation link has no text of its own (textContent empty); its accessible name is an aria-label equal
//     to the old visible label, and hovering or focusing it shows a react-bootstrap tooltip (role=tooltip) with that label
//   - the active item is the router's `aria-current="page"` link (class `active`); "solid red block" is pinned in
//     assets/stylesSpec28.test.ts
//   - the footer toggle shows bi-moon-fill in light mode and bi-brightness-high-fill in dark mode (P4TG's FooterButtons)
// The existing layout.test.tsx keeps pinning names, order, aria-current, the docs link and "no localStorage write".

const LABELS = ['Dashboard', 'Interfaces', 'Host hints', 'Settings'];
const ICONS: Record<string, string> = {
  Dashboard: 'bi-speedometer',
  Interfaces: 'bi-ethernet',
  'Host hints': 'bi-lightbulb',
  Settings: 'bi-gear-wide-connected',
};

const nav = () => screen.getByRole('navigation', { name: 'Main navigation' });
const navLink = (name: string) => within(nav()).getByRole('link', { name });

describe('sidebar (spec 28): icon only', () => {
  it.each(LABELS)('"%s" is an icon without visible text, named by aria-label', (name) => {
    renderApp('/');
    const a = navLink(name);
    expect(a).toHaveAttribute('aria-label', name);
    expect(a.textContent?.trim()).toBe('');
    expect(a.querySelector(`i.bi.${ICONS[name]}`)).not.toBeNull();
    expect(a.querySelector('.nav-label')).toBeNull();
  });

  it('no navigation link carries a visible text label', () => {
    renderApp('/');
    expect(nav().textContent?.trim()).toBe('');
    expect(nav().querySelectorAll('.nav-label')).toHaveLength(0);
  });

  it('icons stay hidden from assistive technology (the aria-label names the link)', () => {
    renderApp('/');
    for (const name of LABELS) {
      for (const icon of Array.from(navLink(name).querySelectorAll('i')))
        expect(icon).toHaveAttribute('aria-hidden', 'true');
    }
  });

  // spec 41 (user 2026-11-05): "don't show the page name when hovering above the icons": no tooltip, no visible text
  it.each(LABELS)('hovering "%s" shows nothing: no tooltip, no text; the aria-label stays', async (name) => {
    const user = userEvent.setup();
    renderApp('/');
    await user.hover(navLink(name));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    expect(navLink(name)).toHaveAttribute('aria-label', name);
    expect(nav().textContent?.trim()).toBe('');
    expect(navLink(name)).not.toHaveAttribute('aria-describedby');
  });

  it('keyboard focus shows no tooltip either, but still lands on a named link', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await user.tab();
    // spec 50 F: the first stop is now the logo link, the nav links follow
    expect(must(document.activeElement as HTMLElement | null, 'focused element').getAttribute('aria-label')).toBe(
      'DPDS on GitHub',
    );
    await user.tab();
    const focused = must(document.activeElement as HTMLElement | null, 'focused element');
    expect(LABELS).toContain(focused.getAttribute('aria-label') ?? '');
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });

  it('navigating still works from the icons', async () => {
    const user = userEvent.setup();
    renderApp('/');
    await user.click(navLink('Interfaces'));
    expect(await screen.findByRole('heading', { level: 1, name: 'Interfaces' })).toBeInTheDocument();
  });

  it('exactly the current page link is marked: aria-current and the active class', () => {
    renderApp('/settings');
    for (const name of LABELS) {
      const a = navLink(name);
      if (name === 'Settings') {
        expect(a).toHaveAttribute('aria-current', 'page');
        expect(a).toHaveClass('active');
      } else {
        expect(a).not.toHaveAttribute('aria-current');
        expect(a).not.toHaveClass('active');
      }
    }
  });

  it('the mark follows the navigation', async () => {
    const user = userEvent.setup();
    renderApp('/');
    expect(navLink('Dashboard')).toHaveClass('active');
    await user.click(navLink('Settings'));
    await waitFor(() => expect(navLink('Settings')).toHaveClass('active'));
    expect(navLink('Settings')).toHaveAttribute('aria-current', 'page');
    expect(navLink('Dashboard')).not.toHaveClass('active');
  });
});

describe('sidebar footer (spec 28)', () => {
  const toggle = () => screen.getByRole('button', { name: 'Toggle dark mode' });

  it('light mode shows bi-moon-fill (switch to dark), not the old sun/moon-stars icons', () => {
    setPrefersDark(false);
    renderApp('/');
    expect(toggle().querySelector('i.bi.bi-moon-fill')).not.toBeNull();
    expect(toggle().querySelector('.bi-brightness-high-fill')).toBeNull();
    expect(toggle().querySelector('.bi-sun, .bi-moon-stars')).toBeNull();
  });

  it('dark mode shows bi-brightness-high-fill', () => {
    setPrefersDark(true);
    renderApp('/');
    expect(toggle().querySelector('i.bi.bi-brightness-high-fill')).not.toBeNull();
    expect(toggle().querySelector('.bi-moon-fill')).toBeNull();
  });

  it('toggling swaps the icon, keeps the accessible name and aria-pressed, and persists the choice', async () => {
    const user = userEvent.setup();
    setPrefersDark(false);
    renderApp('/');
    await user.click(toggle());
    expect(toggle().querySelector('.bi-brightness-high-fill')).not.toBeNull();
    expect(toggle()).toHaveAttribute('aria-pressed', 'true');
    expect(localStorage.getItem('theme')).toBe('dark');
    await user.click(toggle());
    expect(toggle().querySelector('.bi-moon-fill')).not.toBeNull();
    expect(toggle()).toHaveAttribute('aria-pressed', 'false');
  });

  it('the toggle icon is hidden from assistive technology', () => {
    renderApp('/');
    expect(must(toggle().querySelector('i'), 'icon')).toHaveAttribute('aria-hidden', 'true');
  });

  it('the API docs link keeps /api/docs, bi-book and its accessible name', () => {
    renderApp('/');
    const a = screen.getByRole('link', { name: 'API documentation' });
    expect(a).toHaveAttribute('href', '/api/docs');
    expect(a.querySelector('i.bi.bi-book')).not.toBeNull();
  });

  it('the toggle comes before the docs link, both in the sidebar footer, after the navigation', () => {
    renderApp('/');
    const docs = screen.getByRole('link', { name: 'API documentation' });
    const footer = must(toggle().closest('.sidebar-footer'), 'sidebar footer');
    expect(footer).toContainElement(docs);
    expect(toggle().compareDocumentPosition(docs) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(nav().compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('no localStorage write on first load', () => {
    setPrefersDark(true);
    renderApp('/');
    expect(localStorage.getItem('theme')).toBeNull();
    expect(localStorage.length).toBe(0);
  });

  it('the version line stays', async () => {
    renderApp('/');
    expect(await screen.findByText(/v1\.0\.0/)).toBeInTheDocument();
  });
});
