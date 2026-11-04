import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { linkPage } from '../test/configPages';
import { EDGE_LINKS, INVALID_LINKS } from '../test/invalidLinks';
import {
  STORAGE_KEY,
  emptyDoc,
  rawStore,
  renderApp,
  seedRaw,
  seedStore,
  storedDoc,
  storedOrEmpty,
} from '../test/helpers';

// Contract: "Local document" and "Corrupt storage" in the v22 block of src/test/helpers.tsx.
// Never silently drop what is stored: a value that cannot be read raises an alert with a reset button, and stays untouched until
// the user chooses to reset.

const CORRUPT: [string, string][] = [
  ['not JSON', '{not json'],
  ['an empty string', ''],
  ['JSON null', 'null'],
  ['a JSON array', '[]'],
  ['an unknown version', JSON.stringify({ version: 2, links: [], settings: fx.settings() })],
  ['version 0', JSON.stringify({ version: 0, links: [], settings: fx.settings() })],
  ['a missing version', JSON.stringify({ links: [], settings: fx.settings() })],
  ['links that are not a list', JSON.stringify({ version: 1, links: {}, settings: fx.settings() })],
  ['missing settings', JSON.stringify({ version: 1, links: [] })],
];

const alertOf = async () => screen.findByRole('alert');
const resetButton = () => screen.getByRole('button', { name: /^reset/i });

describe('local document: corrupt or unknown storage', () => {
  it.each(CORRUPT)('%s raises an alert on the dashboard that offers a reset', async (_name, raw) => {
    seedRaw(raw);
    renderApp('/');
    const alert = await alertOf();
    expect(alert).toHaveTextContent(/local configuration/i);
    expect(within(alert).getByRole('button', { name: /^reset/i })).toBeInTheDocument();
  });

  it.each(['/', '/links', '/settings', '/hints', '/links/new'])('the alert shows on %s too', async (path) => {
    seedRaw('{not json');
    renderApp(path);
    expect(await alertOf()).toHaveTextContent(/local configuration/i);
    expect(resetButton()).toBeInTheDocument();
  });

  it('shows the alert without any user action and leaves the stored string untouched', async () => {
    seedRaw('{"version":1,"links":"oops"');
    renderApp('/links');
    await alertOf();
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(rawStore()).toBe('{"version":1,"links":"oops"');
  });

  it('an unknown version is kept as it is (it may come from a newer release), not downgraded or dropped', async () => {
    const raw = JSON.stringify({ version: 7, links: [fx.link()], settings: fx.settings() });
    seedRaw(raw);
    renderApp('/');
    await alertOf();
    expect(rawStore()).toBe(raw);
  });

  it('Start is disabled while the storage is corrupt and nothing is started', async () => {
    seedRaw('{not json');
    const user = userEvent.setup();
    renderApp('/');
    await alertOf();
    const start = await screen.findByRole('button', { name: /^start$/i });
    expect(start).toBeDisabled();
    await user.click(start);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('the Links page does not pretend there are no links: it shows the alert, not the empty state, and no table', async () => {
    seedRaw('{not json');
    renderApp('/links');
    await alertOf();
    expect(screen.queryByText(/no links configured/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
  });

  it('the reset button replaces the value by an empty document (or removes it) and the alert goes away', async () => {
    seedRaw('{not json');
    const user = userEvent.setup();
    renderApp('/links');
    await alertOf();
    await user.click(resetButton());
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(storedOrEmpty()).toEqual(emptyDoc());
    expect(await screen.findByText(/no links/i)).toBeInTheDocument();
  });

  it('after the reset the app works: Start is enabled again', async () => {
    seedRaw(JSON.stringify({ version: 3 }));
    const user = userEvent.setup();
    renderApp('/');
    await alertOf();
    await user.click(resetButton());
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole('button', { name: /^start$/i })).toBeEnabled());
  });

  it('a reset is not offered for a healthy document and nothing is shown for an absent one', async () => {
    seedStore({ links: [fx.link()] });
    const view = renderApp('/');
    await screen.findByTestId('state-badge');
    expect(screen.queryByRole('button', { name: /^reset/i })).not.toBeInTheDocument();
    view.unmount();
    localStorage.removeItem(STORAGE_KEY);
    renderApp('/');
    await screen.findByTestId('state-badge');
    expect(screen.queryByRole('button', { name: /^reset/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('other keys (the theme) are not touched by a reset', async () => {
    localStorage.setItem('theme', 'dark');
    seedRaw('{not json');
    const user = userEvent.setup();
    renderApp('/');
    await alertOf();
    await user.click(resetButton());
    expect(localStorage.getItem('theme')).toBe('dark');
  });
});

describe('local document: what is valid', () => {
  it('a valid document with extra unknown members is accepted without an alert', async () => {
    seedStore({ links: [fx.link()] });
    const stored = JSON.parse(rawStore() ?? 'null') as object;
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...stored, future: { a: 1 } }));
    renderApp('/links');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('an empty valid document shows the empty states without an alert', async () => {
    seedStore();
    renderApp('/links');
    expect(await screen.findByText(/no links/i)).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('opening any page with nothing stored never creates the document', async () => {
    for (const path of ['/', '/links', '/settings', '/hints', '/interfaces', '/links/new']) {
      const view = renderApp(path);
      await waitFor(() => expect(document.querySelector('h1')).not.toBeNull());
      expect(rawStore(), path).toBeNull();
      view.unmount();
    }
  });

  it('the pages read the document at mount: a stored link from an earlier session is listed with its config', async () => {
    seedStore({ links: [fx.linkRev()] });
    renderApp('/links');
    expect(await screen.findByText('b-a')).toBeInTheDocument();
    expect(screen.getByText('policing 40 Gbit/s')).toBeInTheDocument();
  });
});

describe('spec27: the document has no presets member', () => {
  const noPresets = (links = [fx.link()]) => JSON.stringify({ version: 1, links, settings: fx.settings() });

  it('a document without a presets member is a valid document: no alert, its links are listed', async () => {
    seedRaw(noPresets());
    renderApp('/links');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^reset/i })).not.toBeInTheDocument();
  });

  it('a document without a presets member is startable (Start is enabled)', async () => {
    seedRaw(noPresets());
    renderApp('/');
    await waitFor(() => expect(screen.getByRole('button', { name: /^start$/i })).toBeEnabled());
  });

  it('an extra well-formed presets member is accepted and ignored (no alert, links listed)', async () => {
    const preset = { id: 1, name: 'wan', description: 'old', config: fx.config3() };
    seedRaw(JSON.stringify({ version: 1, links: [fx.link()], presets: [preset], settings: fx.settings() }));
    renderApp('/links');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('the first save from an empty store writes {version, links, settings} and no presets member', async () => {
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await linkPage.save(user);
    await screen.findByRole('heading', { level: 1, name: 'Links' });
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'settings', 'version']);
  });

  it('saving the settings into a document without presets does not add the member either', async () => {
    seedRaw(noPresets());
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: /^save$/i }));
    await screen.findByRole('status');
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'settings', 'version']);
  });

  it('deleting a link from a document without presets does not add the member either', async () => {
    seedRaw(noPresets());
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Delete a-b' }));
    await user.click(screen.getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(storedDoc().links).toEqual([]));
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'settings', 'version']);
  });
});

// Spec 27b item 1: a link is valid only if the link editor would accept it unchanged. A stored document with an invalid link is
// corrupt like any other unreadable value: the alert with Reset, the stored text untouched, no page that throws.
describe('spec27b: a stored document with an invalid link is corrupt', () => {
  const docWith = (...links: unknown[]) => JSON.stringify({ version: 1, links, settings: fx.settings() });

  it.each(INVALID_LINKS)('$label: the alert with Reset on /links, the text untouched, no table', async ({ link }) => {
    const raw = docWith(link);
    seedRaw(raw);
    renderApp('/links');
    const alert = await alertOf();
    expect(alert).toHaveTextContent(/local configuration/i);
    expect(within(alert).getByRole('button', { name: /^reset/i })).toBeInTheDocument();
    expect(screen.queryByRole('table')).not.toBeInTheDocument();
    expect(screen.queryByText(/no links configured/i)).not.toBeInTheDocument();
    expect(rawStore()).toBe(raw);
  });

  it.each(['/', '/links', '/settings', '/hints', '/links/new', '/links/1'])(
    'an empty config ({}) raises the alert on %s too and nothing throws',
    async (path) => {
      seedRaw(docWith({ ...fx.link(), config: {} }));
      renderApp(path);
      expect(await alertOf()).toHaveTextContent(/local configuration/i);
      expect(resetButton()).toBeInTheDocument();
    },
  );

  it('one invalid link among valid ones makes the whole document corrupt (nothing is listed)', async () => {
    seedRaw(docWith(fx.link(), { ...fx.loopLink(), config: {} }, fx.linkRev()));
    renderApp('/links');
    await alertOf();
    expect(screen.queryByText('a-b')).not.toBeInTheDocument();
    expect(screen.queryByText('b-a')).not.toBeInTheDocument();
  });

  it('Start is disabled and nothing is started while the document is corrupt this way', async () => {
    seedRaw(docWith({ ...fx.link(), config: {} }));
    const user = userEvent.setup();
    renderApp('/');
    await alertOf();
    const start = await screen.findByRole('button', { name: /^start$/i });
    expect(start).toBeDisabled();
    await user.click(start);
    expect(api.startEmulation).not.toHaveBeenCalled();
  });

  it('Reset removes it: the alert goes away and the app starts from the empty document', async () => {
    seedRaw(docWith({ ...fx.link(), config: {} }));
    const user = userEvent.setup();
    renderApp('/links');
    await alertOf();
    await user.click(resetButton());
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(storedOrEmpty()).toEqual(emptyDoc());
    expect(await screen.findByText(/no links/i)).toBeInTheDocument();
  });

  it('the new-link editor shows the alert and leaves the stored text untouched', async () => {
    const raw = docWith({ ...fx.link(), config: {} });
    seedRaw(raw);
    renderApp('/links/new');
    await alertOf();
    expect(rawStore()).toBe(raw);
  });

  it.each(EDGE_LINKS)('%s is still a valid link: no alert, the link is listed', async (_label, link) => {
    seedRaw(docWith(link));
    renderApp('/links');
    expect(await screen.findByText(String(link.name))).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('the fixtures every other test seeds (a-b, b-a, loop) are valid', async () => {
    seedRaw(docWith(fx.link(), fx.linkRev(), fx.loopLink()));
    renderApp('/links');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.getByText('b-a')).toBeInTheDocument();
    expect(screen.getByText('loop')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});

// Spec 27b item 4: writes store exactly { version: 1, links, settings }. A legacy `presets` member (or any unknown one) of the
// stored document is dropped on the next write; reading alone still leaves the stored text alone.
describe('spec27b: every write stores exactly version, links and settings', () => {
  const KEYS = ['links', 'settings', 'version'];
  const legacy = (links: unknown[] = [fx.loopLink()]) =>
    JSON.stringify({
      version: 1,
      links,
      presets: [{ id: 4, name: 'wan', config: fx.config3() }],
      future: { a: 1 },
      settings: fx.settings({ main_lcore: 2 }),
    });

  it('reading alone leaves the legacy members in the stored text', async () => {
    const raw = legacy();
    seedRaw(raw);
    renderApp('/links');
    expect(await screen.findByText('loop')).toBeInTheDocument();
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(rawStore()).toBe(raw);
  });

  it('saving a new link drops them', async () => {
    seedRaw(legacy());
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await linkPage.save(user);
    await screen.findByRole('heading', { level: 1, name: 'Links' });
    expect(Object.keys(storedDoc()).sort()).toEqual(KEYS);
    expect(storedDoc().links.map((l) => l.name)).toEqual(['loop', 'my-link']);
    expect(storedDoc().settings).toEqual(fx.settings({ main_lcore: 2 }));
  });

  it('saving an edited link drops them', async () => {
    seedRaw(legacy([fx.link()]));
    const user = userEvent.setup();
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('a-b'));
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('heading', { level: 1, name: 'Links' });
    expect(Object.keys(storedDoc()).sort()).toEqual(KEYS);
    expect(storedDoc().links).toEqual([fx.link()]);
  });

  it('deleting a link drops them', async () => {
    seedRaw(legacy([fx.link(), fx.loopLink()]));
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Delete a-b' }));
    await user.click(screen.getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(storedDoc().links.map((l) => l.name)).toEqual(['loop']));
    expect(Object.keys(storedDoc()).sort()).toEqual(KEYS);
    expect(storedDoc().settings).toEqual(fx.settings({ main_lcore: 2 }));
  });

  it('saving the settings drops them', async () => {
    seedRaw(legacy());
    const user = userEvent.setup();
    renderApp('/settings');
    await user.type(await screen.findByLabelText('Poll interval (ms)'), '0');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('status');
    expect(Object.keys(storedDoc()).sort()).toEqual(KEYS);
    expect(storedDoc().links).toEqual([fx.loopLink()]);
    expect(storedDoc().settings.poll_interval_ms).toBe(5000);
  });

  it('"Load running configuration" drops them', async () => {
    seedRaw(legacy([]));
    api.getEmulation.mockResolvedValue(
      fx.emulation('running', { config: fx.runningConfig([fx.link()], fx.settings({ poll_interval_ms: 900 })) }),
    );
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(storedDoc().links).toHaveLength(1));
    expect(Object.keys(storedDoc()).sort()).toEqual(KEYS);
    expect(storedDoc().settings.poll_interval_ms).toBe(900);
  });
});

// Spec 27b item 5: the corrupt-config alert tells where the stored text can be saved.
describe('spec27b: the corrupt-config alert points to Export on the Settings page', () => {
  it.each(['{not json', JSON.stringify({ version: 7, links: [], settings: fx.settings() })])(
    'for the stored text %s',
    async (raw) => {
      seedRaw(raw);
      renderApp('/');
      expect(await alertOf()).toHaveTextContent(/Export on the Settings page saves the stored text/i);
    },
  );

  it.each(['/links', '/settings'])('on %s too', async (path) => {
    seedRaw('{not json');
    renderApp(path);
    expect(await alertOf()).toHaveTextContent(/Export on the Settings page saves the stored text/i);
  });
});

describe('no REST calls for the removed routes', () => {
  it('walking through every page requests nothing but online, interfaces, hints, emulation, stats and histograms', async () => {
    seedStore({ links: [fx.link()] });
    for (const path of ['/', '/links', '/links/1', '/links/new', '/settings', '/hints', '/interfaces']) {
      const view = renderApp(path);
      await waitFor(() => expect(document.querySelector('h1')).not.toBeNull());
      view.unmount();
    }
    expect(Object.keys(api).sort()).toEqual([
      'getDelayHistogram',
      'getEmulation',
      'getHostHints',
      'getHostHintsFor',
      'getInterfaces',
      'getOnline',
      'getStats',
      'startEmulation',
      'stopEmulation',
    ]);
    // the pages did not use fetch directly either (setup stubs it as unreachable)
    expect(vi.mocked(fetch)).not.toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/(links|presets|settings|export|import)/),
      expect.anything(),
    );
  });
});
