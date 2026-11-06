// The sidebar shows `v<version>` and tells when a newer release is on main, like P4TG
// (`isUpdateAvailable` in its Helper.ts, the update modal in App.tsx, the arrow in Navbar.tsx).
//
// Contract these tests pin:
// - After the first successful `/api/online` the frontend runs ONE check per page load: global
//   `fetch('https://raw.githubusercontent.com/uni-tue-kn/DPDS/refs/heads/main/VERSION.md',
//   { cache: 'no-store', ... })`. The API client is mocked in these tests, so every `fetch` call is the check.
// - Parsing as P4TG: both versions must match `^\s*v?(\d+)\.(\d+)\.(\d+)\s*$` (the remote one on its first line
//   only, split on CRLF or LF); major, minor and patch are compared as numbers. Not ok (HTTP status), a rejected or
//   throwing fetch, a failing `text()`, a malformed remote or local version, an equal or older remote: no update.
// - Update available: a link with the accessible name "New DPDS release available" to
//   https://github.com/uni-tue-kn/DPDS/releases (new tab, noreferrer/noopener, class `text-danger`, icon
//   `bi-arrow-up-circle-fill`) after the version text in the sidebar, with the tooltip "New release available"
//   (a `title` or an overlay with role tooltip); and ONE modal (role dialog) "Update available" with the text
//   "A new stable DPDS release is available. Update with ./dpds.sh update --stable (see the releases page).",
//   a "Close" button and "Open releases" (a link to the releases page, or a button that opens it).
// - The modal appears once per page load: closing it keeps it closed through later polls and navigation. The
//   arrow stays. "Offline" has no arrow.
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import { api } from './test/mockApi';
import * as fx from './test/fixtures';
import { renderApp, tick } from './test/helpers';

const VERSION_URL = 'https://raw.githubusercontent.com/uni-tue-kn/DPDS/refs/heads/main/VERSION.md';
const RELEASES_URL = 'https://github.com/uni-tue-kn/DPDS/releases';
const MODAL_TEXT =
  'A new stable DPDS release is available. Update with ./dpds.sh update --stable (see the releases page).';

type Remote = { ok?: boolean; status?: number; body: string };

let fetchMock: ReturnType<typeof vi.fn>;

function respond(r: Remote) {
  return { ok: r.ok ?? true, status: r.status ?? 200, text: async () => r.body };
}

function remoteIs(body: string, extra: Partial<Remote> = {}) {
  fetchMock.mockImplementation(async () => respond({ body, ...extra }));
}

beforeEach(() => {
  fetchMock = vi.fn();
  vi.stubGlobal('fetch', fetchMock);
  remoteIs('1.0.0\n');
});

const arrow = () => screen.queryByRole('link', { name: 'New DPDS release available', hidden: true });
const modal = () => screen.queryByRole('dialog');

/** Lets promise chains and React effects finish (real timers). */
async function settle(ms = 40) {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

async function renderChecked(local: string, remote: string, extra: Partial<Remote> = {}) {
  api.getOnline.mockResolvedValue(fx.online({ version: local }));
  remoteIs(remote, extra);
  const view = renderApp('/hints');
  await screen.findByText(new RegExp(`v${local.replace(/\./g, '\\.')}`));
  await waitFor(() => expect(fetchMock).toHaveBeenCalled());
  await settle();
  return view;
}

describe('the version line (spec 18)', () => {
  it('shows v<version> from /api/online in the sidebar below the navigation', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    renderApp('/hints');
    const v = await screen.findByText(/^v1\.0\.0/);
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(v.closest('.sidebar')).toBe(nav.closest('.sidebar'));
    expect(nav.compareDocumentPosition(v) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(nav).not.toContainElement(v);
  });

  it('shows whatever version the controller reports', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '2.14.7' }));
    renderApp('/hints');
    expect(await screen.findByText(/v2\.14\.7/)).toBeInTheDocument();
  });

  it('shows Offline when /api/online is unreachable, without an update arrow', async () => {
    api.getOnline.mockRejectedValue(new Error('Failed to fetch'));
    remoteIs('9.9.9\n');
    renderApp('/hints');
    expect(await screen.findByText('Offline')).toBeInTheDocument();
    await settle();
    expect(screen.queryByText(/^v\d/)).not.toBeInTheDocument();
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });
});

describe('the update check request (spec 18)', () => {
  it('fetches the VERSION.md of main on raw.githubusercontent.com with cache: no-store', async () => {
    await renderChecked('1.0.0', '1.0.0\n');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(String(url)).toBe(VERSION_URL);
    expect(init).toEqual(expect.objectContaining({ cache: 'no-store' }));
  });

  it('asks the browser for GitHub only, never the DPDS host (no request through the API client)', async () => {
    await renderChecked('1.0.0', '1.0.0\n');
    for (const [url] of fetchMock.mock.calls) expect(String(url)).toMatch(/^https:\/\/raw\.githubusercontent\.com\//);
  });

  it('checks once per page load, not once per poll', async () => {
    vi.useFakeTimers();
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    renderApp('/hints');
    await tick(0);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const polls = api.getOnline.mock.calls.length;
    await tick(30000);
    expect(api.getOnline.mock.calls.length).toBeGreaterThanOrEqual(polls + 4);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('does not check again after navigating to another page', async () => {
    const user = userEvent.setup();
    await renderChecked('1.0.0', '1.0.0\n');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    await user.click(within(nav).getByRole('link', { name: 'Settings' }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    await settle();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});

// [local, remote body, update expected]
const COMPARE: [string, string, boolean][] = [
  ['1.0.0', '1.0.1\n', true],
  ['1.0.0', '1.1.0\n', true],
  ['1.0.0', '2.0.0\n', true],
  ['1.2.3', '1.2.3\n', false],
  ['1.0.1', '1.0.0\n', false],
  ['1.1.0', '1.0.9\n', false],
  ['2.0.0', '1.9.9\n', false],
  ['1.9.0', '1.10.0\n', true],
  ['1.10.0', '1.9.0\n', false],
  ['1.0.9', '1.0.10\n', true],
  ['0.9.9', '1.0.0\n', true],
  ['10.0.0', '9.0.0\n', false],
];

// the same comparison with the remote text in the shapes P4TG accepts
const REMOTE_FORMS: [string, string, boolean][] = [
  ['v prefix', 'v1.0.1\n', true],
  ['no trailing newline', '1.0.1', true],
  ['surrounding white space', '  1.0.1  \n', true],
  ['CRLF', '1.0.1\r\n', true],
  ['only the first line counts (newer)', '1.0.1\nsecond line 9.9.9\n', true],
  ['only the first line counts (equal)', '1.0.0\n9.9.9\n', false],
  ['v prefix, equal', 'v1.0.0\n', false],
];

// a malformed remote never means an update, however newer it looks
const MALFORMED: [string, string][] = [
  ['empty', ''],
  ['blank first line', '\n2.0.0\n'],
  ['text', 'banana\n'],
  ['two parts', '2.0\n'],
  ['four parts', '2.0.0.0\n'],
  ['a letter in the patch', '2.0.x\n'],
  ['a pre-release suffix', '2.0.0-rc1\n'],
  ['an upper case V', 'V2.0.0\n'],
  ['a 404 page', '404: Not Found\n'],
  ['HTML', '<html>2.0.0</html>\n'],
  ['negative numbers', '2.-1.0\n'],
];

describe('the version comparison (spec 18, as P4TG)', () => {
  it.each(COMPARE)('local %s, remote %j: update %s', async (local, remote, expected) => {
    await renderChecked(local, remote);
    expect(arrow() !== null).toBe(expected);
    expect(modal() !== null).toBe(expected);
  });

  it.each(REMOTE_FORMS)('remote with %s (%j): update %s', async (_what, remote, expected) => {
    await renderChecked('1.0.0', remote);
    expect(arrow() !== null).toBe(expected);
    expect(modal() !== null).toBe(expected);
  });

  it.each(MALFORMED)('a malformed remote (%s) is no update', async (_what, remote) => {
    await renderChecked('1.0.0', remote);
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });

  it('a non-ok answer is no update even if the body holds a newer version', async () => {
    await renderChecked('1.0.0', '9.9.9\n', { ok: false, status: 404 });
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });

  it('a rate-limited or failing server (500, 429) is no update', async () => {
    for (const status of [429, 500]) {
      const view = await renderChecked('1.0.0', '9.9.9\n', { ok: false, status });
      expect(arrow(), String(status)).toBeNull();
      expect(modal(), String(status)).toBeNull();
      view.unmount();
    }
  });

  it('a network error is no update and breaks nothing', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    fetchMock.mockRejectedValue(new TypeError('Failed to fetch'));
    renderApp('/hints');
    expect(await screen.findByText(/v1\.0\.0/)).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await settle();
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
    expect(screen.getByText(/v1\.0\.0/)).toBeInTheDocument();
  });

  it('a fetch that throws at once (no fetch available) is no update', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    fetchMock.mockImplementation(() => {
      throw new Error('fetch is not available');
    });
    renderApp('/hints');
    expect(await screen.findByText(/v1\.0\.0/)).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await settle();
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });

  it('a body that cannot be read is no update', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    fetchMock.mockResolvedValue({
      ok: true,
      status: 200,
      text: async () => {
        throw new Error('stream failed');
      },
    });
    renderApp('/hints');
    expect(await screen.findByText(/v1\.0\.0/)).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await settle();
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });

  it.each(['dev', '', '1.0', 'v1.0.0.0'])('a malformed local version (%j) is no update', async (local) => {
    api.getOnline.mockResolvedValue(fx.online({ version: local }));
    remoteIs('9.9.9\n');
    renderApp('/hints');
    await waitFor(() => expect(api.getOnline).toHaveBeenCalled());
    await settle(80);
    expect(arrow()).toBeNull();
    expect(modal()).toBeNull();
  });
});

describe('the update arrow (spec 18, as P4TG)', () => {
  async function withUpdate() {
    return renderChecked('1.0.0', '1.0.1\n');
  }

  it('is a red arrow link to the releases page that opens a new tab safely', async () => {
    await withUpdate();
    const a = arrow() as HTMLElement;
    expect(a).not.toBeNull();
    expect(a.tagName).toBe('A');
    expect(a).toHaveAttribute('href', RELEASES_URL);
    expect(a).toHaveAttribute('target', '_blank');
    const rel = (a.getAttribute('rel') ?? '').split(/\s+/);
    expect(rel.includes('noreferrer') || rel.includes('noopener')).toBe(true);
    expect(a.className).toMatch(/\btext-danger\b/);
    const icon = a.querySelector('i.bi.bi-arrow-up-circle-fill');
    expect(icon).not.toBeNull();
  });

  it('sits in the sidebar after the version text', async () => {
    await withUpdate();
    const a = arrow() as HTMLElement;
    const v = screen.getByText(/v1\.0\.0/, { ignore: 'script' });
    expect(a.closest('.sidebar')).toBe(v.closest('.sidebar'));
    expect(v.compareDocumentPosition(a) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const nav = screen.getByRole('navigation', { name: 'Main navigation', hidden: true });
    expect(nav).not.toContainElement(a);
  });

  it('has the tooltip "New release available"', async () => {
    const user = userEvent.setup();
    await withUpdate();
    await user.click(within(modal() as HTMLElement).getAllByRole('button', { name: 'Close' })[0]);
    await waitFor(() => expect(modal()).toBeNull());
    const a = arrow() as HTMLElement;
    if (a.getAttribute('title') === 'New release available') return;
    await user.hover(a);
    const tip = await screen.findByRole('tooltip');
    expect(tip).toHaveTextContent('New release available');
  });

  it('is absent without an update, and does not mention one anywhere', async () => {
    await renderChecked('1.0.0', '1.0.0\n');
    expect(arrow()).toBeNull();
    expect(screen.queryByText(/new release available/i)).not.toBeInTheDocument();
    expect(screen.queryByText('Update available')).not.toBeInTheDocument();
  });

  it('stays after the modal was closed', async () => {
    const user = userEvent.setup();
    await withUpdate();
    await user.click(within(modal() as HTMLElement).getAllByRole('button', { name: 'Close' })[0]);
    await waitFor(() => expect(modal()).toBeNull());
    expect(arrow()).not.toBeNull();
    expect(screen.getByRole('link', { name: 'New DPDS release available' })).toHaveAttribute('href', RELEASES_URL);
  });

  it('turns into Offline without the arrow when the controller goes away', async () => {
    vi.useFakeTimers();
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    remoteIs('1.0.1\n');
    renderApp('/hints');
    await tick(0);
    expect(arrow()).not.toBeNull();
    act(() => {
      fireEvent.click(within(modal() as HTMLElement).getAllByRole('button', { name: 'Close' })[0]);
    });
    await tick(1000);
    api.getOnline.mockRejectedValue(new Error('Failed to fetch'));
    await tick(11000);
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(arrow()).toBeNull();
  });
});

describe('the update modal (spec 18, as P4TG)', () => {
  it('opens once the check finds a newer version: title, text, Close and Open releases', async () => {
    await renderChecked('1.0.0', '1.0.1\n');
    const m = modal() as HTMLElement;
    expect(m).not.toBeNull();
    expect(within(m).getByText('Update available')).toBeInTheDocument();
    expect(m.textContent?.replace(/\s+/g, ' ')).toContain(MODAL_TEXT);
    const closers = within(m).getAllByRole('button', { name: 'Close' });
    expect(closers.some((b) => b.textContent?.trim() === 'Close')).toBe(true);
    const open =
      within(m).queryByRole('link', { name: 'Open releases' }) ??
      within(m).getByRole('button', { name: 'Open releases' });
    expect(open).toBeInTheDocument();
  });

  it('"Open releases" leads to the releases page in a new tab', async () => {
    const user = userEvent.setup();
    const opened = vi.fn();
    vi.stubGlobal('open', opened);
    window.open = opened as unknown as typeof window.open;
    await renderChecked('1.0.0', '1.0.1\n');
    const m = modal() as HTMLElement;
    // react-bootstrap renders `<Button href>` as an anchor with role="button", so look for either role
    const open = (within(m).queryByRole('link', { name: 'Open releases' }) ??
      within(m).getByRole('button', { name: 'Open releases' })) as HTMLElement;
    if (open.tagName === 'A' && open.hasAttribute('href')) {
      expect(open).toHaveAttribute('href', RELEASES_URL);
      expect(open).toHaveAttribute('target', '_blank');
      const rel = (open.getAttribute('rel') ?? '').split(/\s+/);
      expect(rel.includes('noreferrer') || rel.includes('noopener')).toBe(true);
    } else {
      await user.click(open);
      expect(opened).toHaveBeenCalledTimes(1);
      expect(String(opened.mock.calls[0][0])).toBe(RELEASES_URL);
    }
  });

  it('Close closes it', async () => {
    const user = userEvent.setup();
    await renderChecked('1.0.0', '1.0.1\n');
    const m = modal() as HTMLElement;
    const footer = within(m)
      .getAllByRole('button', { name: 'Close' })
      .find((b) => b.textContent?.trim() === 'Close');
    await user.click(footer as HTMLElement);
    await waitFor(() => expect(modal()).toBeNull());
    expect(screen.queryByText('Update available')).not.toBeInTheDocument();
  });

  it('is not shown without an update', async () => {
    await renderChecked('1.0.0', '1.0.0\n');
    expect(modal()).toBeNull();
    expect(screen.queryByText(MODAL_TEXT)).not.toBeInTheDocument();
  });

  it('is one dialog, also after more polls', async () => {
    vi.useFakeTimers();
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    remoteIs('1.0.1\n');
    renderApp('/hints');
    await tick(0);
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    await tick(30000);
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('once closed it stays closed through later polls', async () => {
    vi.useFakeTimers();
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    remoteIs('1.0.1\n');
    renderApp('/hints');
    await tick(0);
    const m = modal() as HTMLElement;
    const polls = api.getOnline.mock.calls.length;
    act(() => {
      fireEvent.click(within(m).getAllByRole('button', { name: 'Close' })[0]);
    });
    await tick(1000);
    expect(modal()).toBeNull();
    await tick(30000);
    expect(api.getOnline.mock.calls.length).toBeGreaterThan(polls);
    expect(modal()).toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(arrow()).not.toBeNull();
  });

  it('once closed it does not come back when the user opens another page', async () => {
    const user = userEvent.setup();
    await renderChecked('1.0.0', '1.0.1\n');
    await user.click(within(modal() as HTMLElement).getAllByRole('button', { name: 'Close' })[0]);
    await waitFor(() => expect(modal()).toBeNull());
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    await user.click(within(nav).getByRole('link', { name: 'Settings' }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    await user.click(within(nav).getByRole('link', { name: 'Interfaces' }));
    await screen.findByRole('heading', { level: 1, name: 'Interfaces' });
    await settle();
    expect(modal()).toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('opens on any page, not only the dashboard', async () => {
    api.getOnline.mockResolvedValue(fx.online({ version: '1.0.0' }));
    remoteIs('1.0.1\n');
    renderApp('/settings');
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
  });

  it('a new page load checks and asks again', async () => {
    const first = await renderChecked('1.0.0', '1.0.1\n');
    expect(fetchMock).toHaveBeenCalledTimes(1);
    first.unmount();
    renderApp('/hints');
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
  });
});
