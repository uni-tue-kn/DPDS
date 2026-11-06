import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { renderApp } from './test/helpers';
import { must } from './test/must';

// The frontend shows the DPDS logo (README's docs/img/logo.png) instead of the plain text "DPDS".
describe('sidebar brand', () => {
  it('is the DPDS logo image, not the text "DPDS"', () => {
    renderApp('/settings');
    const logo = screen.getByRole('img', { name: 'DPDS logo' });
    expect(logo.tagName).toBe('IMG');
    expect(logo.getAttribute('src')).toMatch(/logo/);
    const brand = must(logo.closest('.sidebar-brand'), 'logo inside .sidebar-brand') as HTMLElement;
    expect(brand.textContent?.trim()).toBe('');
  });

  // Clicking the DPDS logo sends the user to the GitHub repository.
  it('is a link to the GitHub repository that opens in a new tab safely, with an accessible name', () => {
    renderApp('/settings');
    const logo = screen.getByRole('img', { name: 'DPDS logo' });
    const link = must(logo.closest('a'), 'the logo inside a link') as HTMLAnchorElement;
    expect(link.getAttribute('href')).toBe('https://github.com/uni-tue-kn/DPDS');
    expect(link).toHaveAttribute('target', '_blank');
    const rel = (link.getAttribute('rel') ?? '').split(/\s+/);
    expect(rel).toEqual(expect.arrayContaining(['noopener', 'noreferrer']));
    expect(link).toHaveAccessibleName(/github/i);
    expect(link.closest('.sidebar-brand')).not.toBeNull();
  });

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
