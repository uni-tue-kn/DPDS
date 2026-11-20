// The app shell: sidebar navigation, theme, the controller going offline and coming back, the other-users warning and the update check.
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import { ApiError } from './api/client';
import * as fx from './test/fixtures';
import { renderApp, tick } from './test/helpers';
import { setPrefersDark } from './test/matchMedia';
import { api } from './test/mockApi';
import { must } from './test/must';

const PAGES: [name: string, path: string][] = [
  ['Dashboard', '/'],
  ['Interfaces', '/interfaces'],
  ['Host hints', '/hints'],
  ['Settings', '/settings'],
];

const heading = (name: string) => screen.queryByRole('heading', { level: 1, name });
const theme = () => document.documentElement.getAttribute('data-theme');

/** `getOnline` answers while `state.up` and fails like an unreachable controller otherwise. */
function controller(up: boolean) {
  const state = { up };
  api.getOnline.mockImplementation(async () => {
    if (!state.up) throw new TypeError('Failed to fetch');
    return fx.online();
  });
  return state;
}

describe('sidebar navigation', () => {
  it.each(PAGES)('the link "%s" opens %s and marks it as the current page', async (name, path) => {
    const user = userEvent.setup();
    renderApp(path === '/' ? '/hints' : '/');
    await user.click(screen.getByRole('link', { name }));
    expect(await screen.findByRole('heading', { level: 1, name })).toBeInTheDocument();
    expect(screen.getByRole('link', { name })).toHaveAttribute('aria-current', 'page');
  });

  it('an unknown route and the removed routes lead to the dashboard', async () => {
    renderApp('/presets');
    expect(await screen.findByRole('heading', { level: 1, name: 'Dashboard' })).toBeInTheDocument();
  });
});

describe('theme', () => {
  it.each([
    ['follows the system preference', { dark: true, stored: null }, 'dark'],
    ['is light without a preference', { dark: false, stored: null }, 'light'],
    ['prefers the stored choice to the system', { dark: true, stored: 'light' }, 'light'],
  ])('%s', (_name, { dark, stored }, expected) => {
    setPrefersDark(dark);
    if (stored) localStorage.setItem('theme', stored);
    renderApp('/hints');
    expect(theme()).toBe(expected);
  });

  it('the toggle flips the theme, persists the choice and survives a reload', async () => {
    const user = userEvent.setup();
    const first = renderApp('/hints');
    expect(localStorage.getItem('theme')).toBeNull();
    const toggle = screen.getByRole('button', { name: 'Toggle dark mode' });
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await user.click(toggle);
    expect(theme()).toBe('dark');
    expect(toggle).toHaveAttribute('aria-pressed', 'true');
    expect(localStorage.getItem('theme')).toBe('dark');

    first.unmount();
    renderApp('/hints');
    expect(theme()).toBe('dark');
    await user.click(screen.getByRole('button', { name: 'Toggle dark mode' }));
    expect(theme()).toBe('light');
    expect(localStorage.getItem('theme')).toBe('light');
  });
});

describe('controller status', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it('shows the version while the controller answers', async () => {
    controller(true);
    renderApp('/hints');
    await tick(0);
    expect(screen.getByText('v1.0.0')).toBeInTheDocument();
    expect(heading('Host hints')).toBeInTheDocument();
  });

  it('replaces the page by the offline page when the controller is unreachable and brings it back at the same URL', async () => {
    const state = controller(false);
    renderApp('/hints');
    await tick(0);
    expect(screen.getByText('DPDS device not reachable')).toBeInTheDocument();
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(heading('Host hints')).not.toBeInTheDocument();
    await tick(10000);
    expect(screen.getByText('DPDS device not reachable')).toBeInTheDocument();

    state.up = true;
    await tick(2000);
    expect(screen.queryByText('DPDS device not reachable')).not.toBeInTheDocument();
    expect(heading('Host hints')).toBeInTheDocument();
    await tick(5000); // the sidebar's own poll picks the version up
    expect(screen.getByText('v1.0.0')).toBeInTheDocument();
  });

  it('goes offline when a later poll fails', async () => {
    const state = controller(true);
    renderApp('/hints');
    await tick(0);
    state.up = false;
    await tick(5000);
    expect(screen.getByText('DPDS device not reachable')).toBeInTheDocument();
  });

  it('an HTTP error answer is not "offline": the page stays', async () => {
    api.getOnline.mockRejectedValue(new ApiError(500, 'boom'));
    renderApp('/hints');
    await tick(0);
    expect(screen.queryByText('DPDS device not reachable')).not.toBeInTheDocument();
    expect(heading('Host hints')).toBeInTheDocument();
  });

  it('warns about other connected users, follows the count and drops the warning when the controller goes away', async () => {
    api.getOnline
      .mockResolvedValueOnce(fx.online({ connected_clients: 1 }))
      .mockResolvedValueOnce(fx.online({ connected_clients: 3 }))
      .mockResolvedValueOnce(fx.online({ connected_clients: 0 }))
      .mockResolvedValueOnce(fx.online({ connected_clients: 2 }))
      .mockRejectedValue(new TypeError('Failed to fetch'));
    renderApp('/hints');
    await tick(0);
    const warning = () => screen.queryByTestId('session-warning');
    expect(warning()).toHaveTextContent('1 other user is connected');
    await tick(5000);
    expect(warning()).toHaveTextContent('3 other users are connected');
    await tick(5000);
    expect(warning()).not.toBeInTheDocument();
    await tick(5000);
    expect(warning()).toHaveTextContent('2 other users');
    await tick(5000);
    expect(warning()).not.toBeInTheDocument();
  });
});

describe('update check', () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  /** The VERSION.md of main on GitHub says `remote`; the controller runs `local`. */
  function setup(local: string, remote: string) {
    fetchMock = vi.fn(async () => ({ ok: true, status: 200, text: async () => remote }));
    vi.stubGlobal('fetch', fetchMock);
    api.getOnline.mockResolvedValue(fx.online({ version: local }));
  }
  const settle = () => act(async () => void (await new Promise((r) => setTimeout(r, 40))));

  it('a newer release shows the arrow link and one modal; closing it keeps it closed through later polls and pages', async () => {
    const user = userEvent.setup();
    setup('1.0.0', '1.1.0\n');
    renderApp('/hints');
    const dialog = await screen.findByRole('dialog');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0]).toMatch(/raw\.githubusercontent\.com\/.*VERSION\.md$/);
    const releases = within(dialog).getByRole('button', { name: 'Open releases' });
    expect(releases).toHaveAttribute('href', 'https://github.com/uni-tue-kn/DPDS/releases');
    await user.click(must(within(dialog).getAllByRole('button', { name: 'Close' })[0]));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(screen.getByRole('link', { name: 'New DPDS release available', hidden: true })).toHaveAttribute(
      'href',
      releases.getAttribute('href'),
    );

    await user.click(screen.getByRole('link', { name: 'Interfaces' }));
    await settle();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['the same version', '1.0.0', '1.0.0\n'],
    ['an older release', '1.2.0', '1.1.9\n'],
    ['a malformed remote version', '1.0.0', '<html>rate limited</html>'],
    ['a malformed local version', 'dev', '9.9.9\n'],
  ])('%s is no update: no arrow, no modal', async (_name, local, remote) => {
    setup(local, remote);
    renderApp('/hints');
    await screen.findByText(new RegExp(`^v${local}`));
    if (local !== 'dev') await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await settle();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'New DPDS release available', hidden: true })).not.toBeInTheDocument();
  });

  it('checks once per page load, not once per poll', async () => {
    vi.useFakeTimers();
    setup('1.0.0', '1.0.0\n');
    renderApp('/hints');
    await tick(0);
    await tick(20000);
    expect(api.getOnline.mock.calls.length).toBeGreaterThan(3);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('an unreachable GitHub breaks nothing', async () => {
    setup('1.0.0', '');
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    renderApp('/hints');
    await screen.findByText('v1.0.0');
    await settle();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(heading('Host hints')).toBeInTheDocument();
  });
});
