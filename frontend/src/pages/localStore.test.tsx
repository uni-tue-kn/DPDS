import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { STORAGE_KEY, emptyDoc, rawStore, renderApp, seedRaw, seedStore, storedOrEmpty } from '../test/helpers';

// Contract: "Local document" and "Corrupt storage" in the v22 block of src/test/helpers.tsx.
// Never silently drop what is stored: a value that cannot be read raises an alert with a reset button, and stays untouched until
// the user chooses to reset.

const CORRUPT: [string, string][] = [
  ['not JSON', '{not json'],
  ['an empty string', ''],
  ['JSON null', 'null'],
  ['a JSON array', '[]'],
  ['an unknown version', JSON.stringify({ version: 2, links: [], presets: [], settings: fx.settings() })],
  ['version 0', JSON.stringify({ version: 0, links: [], presets: [], settings: fx.settings() })],
  ['a missing version', JSON.stringify({ links: [], presets: [], settings: fx.settings() })],
  ['links that are not a list', JSON.stringify({ version: 1, links: {}, presets: [], settings: fx.settings() })],
  ['presets that are not a list', JSON.stringify({ version: 1, links: [], presets: 'x', settings: fx.settings() })],
  ['missing settings', JSON.stringify({ version: 1, links: [], presets: [] })],
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

  it.each(['/', '/links', '/settings', '/presets', '/hints', '/links/new'])(
    'the alert shows on %s too',
    async (path) => {
      seedRaw('{not json');
      renderApp(path);
      expect(await alertOf()).toHaveTextContent(/local configuration/i);
      expect(resetButton()).toBeInTheDocument();
    },
  );

  it('shows the alert without any user action and leaves the stored string untouched', async () => {
    seedRaw('{"version":1,"links":"oops"');
    renderApp('/links');
    await alertOf();
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(rawStore()).toBe('{"version":1,"links":"oops"');
  });

  it('an unknown version is kept as it is (it may come from a newer release), not downgraded or dropped', async () => {
    const raw = JSON.stringify({ version: 7, links: [fx.link()], presets: [], settings: fx.settings() });
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
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...seedStore({ links: [fx.link()] }), future: { a: 1 } }));
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
    for (const path of [
      '/',
      '/links',
      '/presets',
      '/settings',
      '/hints',
      '/interfaces',
      '/links/new',
      '/presets/new',
    ]) {
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

describe('no REST calls for the removed routes', () => {
  it('walking through every page requests nothing but online, interfaces, hints, emulation, stats and histograms', async () => {
    seedStore({ links: [fx.link()], presets: [fx.preset({ id: 1 })] });
    for (const path of [
      '/',
      '/links',
      '/links/1',
      '/links/new',
      '/presets',
      '/presets/1',
      '/presets/new',
      '/settings',
      '/hints',
      '/interfaces',
    ]) {
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
