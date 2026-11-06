import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Link } from '../api/types';
import { emptyConfig, isLocalConfig, parseStored, startRequestOf, writeConfig } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import * as fx from '../test/fixtures';
import { api } from '../test/mockApi';
import { rawStore, renderApp, seedRaw, storedDoc } from '../test/helpers';
import { must } from '../test/must';

// No settings, no worker, and the sidebar is Dashboard, Interfaces, Host hints, Settings. The Settings page IS the Links page (list,
// Import / Export toolbar, editor under /settings/new and /settings/:id). A stored or imported `settings` member and a link's
// `worker` member are ignored (never an error) and never written back. Start, validate and hints send exactly `{links}`.
// A `worker` is ignored whatever its value; "not written back" is checked after every kind of write (new link, edit, delete,
// Duplicate, import) and for Export; the validate request has no frontend caller, so only start and hints are driven here.
// Import / Export of these members is in linksImportExport.test.tsx; the sidebar details are in sidebar.test.tsx.

/** What an earlier version stored: `settings` at the top and a `worker` on the links. */
const LEGACY_SETTINGS = { main_lcore: 2, eal_extra_args: ['--no-huge', '-m', '256'], poll_interval_ms: 250 };
const legacyLinks = (): Record<string, unknown>[] => [
  { ...fx.link(), worker: 5 },
  { ...fx.linkRev(), worker: 0 },
];
/** The same, on vdev ports, so that the PCI ports are free for a new link or a copy (the editor disables ports in use). */
const legacyVdevDoc = () =>
  JSON.stringify({
    version: 1,
    links: [
      { ...fx.link({ ingress: 'net_null0', egress: 'net_null1' }), worker: 5 },
      { ...fx.linkRev({ ingress: 'net_null2', egress: 'net_null3' }), worker: 0 },
    ],
    settings: LEGACY_SETTINGS,
  });
const legacyDoc = (over: Record<string, unknown> = {}) =>
  JSON.stringify({ version: 1, links: legacyLinks(), settings: LEGACY_SETTINGS, ...over });

const nav = () => screen.getByRole('navigation', { name: 'Main navigation' });
const hasMember = (links: object[], key: string) => links.some((l) => key in l);

describe('sidebar and routes (spec 41)', () => {
  it('holds exactly Dashboard, Interfaces, Host hints, Settings, in this order, and no Links entry', () => {
    renderApp('/');
    const names = within(nav())
      .getAllByRole('link')
      .map((a) => a.getAttribute('aria-label'));
    expect(names).toEqual(['Dashboard', 'Interfaces', 'Host hints', 'Settings']);
    expect(within(nav()).queryByRole('link', { name: 'Links' })).not.toBeInTheDocument();
    expect(document.querySelector('a[href="/links"]')).toBeNull();
  });

  it.each([
    ['Dashboard', '/', 'bi-speedometer'],
    ['Interfaces', '/interfaces', 'bi-ethernet'],
    ['Host hints', '/hints', 'bi-lightbulb'],
    ['Settings', '/settings', 'bi-gear-wide-connected'],
  ])('"%s" links to %s with the icon %s', (name, href, icon) => {
    renderApp('/');
    const a = within(nav()).getByRole('link', { name });
    expect(a).toHaveAttribute('href', href);
    expect(a.querySelector(`i.bi.${icon}`)).not.toBeNull();
    // P4TG's dashboard icon is exactly `bi-speedometer`
    expect(a.querySelector('i.bi-speedometer2')).toBeNull();
    expect(a.querySelector('i.bi-diagram-2')).toBeNull();
  });

  it('/settings is the page with the links: heading "Settings", the link table, a New link button to /settings/new', async () => {
    seedRaw(JSON.stringify({ version: 1, links: [fx.link(), fx.linkRev()] }));
    renderApp('/settings');
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { level: 1, name: 'Links' })).not.toBeInTheDocument();
    const table = await screen.findByRole('table');
    expect(within(table).getByText('a-b')).toBeInTheDocument();
    expect(within(table).getByText('b-a')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /new link/i })).toHaveAttribute('href', '/settings/new');
    expect(screen.getByRole('link', { name: 'Edit a-b' })).toHaveAttribute('href', '/settings/1');
    expect(screen.getByRole('link', { name: 'Edit b-a' })).toHaveAttribute('href', '/settings/3');
  });

  it('the old settings form is gone: no main lcore, no EAL arguments, no poll interval, no Save', async () => {
    renderApp('/settings');
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    for (const label of [/main lcore/i, /poll interval/i, /EAL/i, /lcore/i]) {
      expect(screen.queryByLabelText(label)).not.toBeInTheDocument();
    }
    expect(screen.queryByRole('button', { name: /add eal/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^save$/i })).not.toBeInTheDocument();
    expect(screen.queryByText(/eal (extra )?arguments?/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/poll interval/i)).not.toBeInTheDocument();
  });

  it('/settings/new and /settings/:id are the link editor', async () => {
    seedRaw(JSON.stringify({ version: 1, links: [fx.link()] }));
    const first = renderApp('/settings/new');
    expect(await screen.findByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    first.unmount();
    renderApp('/settings/1');
    expect(await screen.findByRole('heading', { level: 1, name: 'Edit link' })).toBeInTheDocument();
  });

  it.each(['/links', '/links/new', '/links/1'])(
    'the old route %s is gone: it falls through to the dashboard',
    async (path) => {
      seedRaw(JSON.stringify({ version: 1, links: [fx.link()] }));
      renderApp(path);
      expect(await screen.findByRole('heading', { level: 1, name: 'Dashboard' })).toBeInTheDocument();
      expect(screen.queryByRole('heading', { level: 1, name: 'Edit link' })).not.toBeInTheDocument();
      expect(screen.queryByRole('heading', { level: 1, name: 'New link' })).not.toBeInTheDocument();
      expect(screen.queryByRole('table')).not.toBeInTheDocument();
    },
  );

  it('the editor Cancel and a save lead back to /settings', async () => {
    const user = userEvent.setup();
    seedRaw(JSON.stringify({ version: 1, links: [fx.link()] }));
    renderApp('/settings/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    expect(screen.getByRole('link', { name: 'Cancel' })).toHaveAttribute('href', '/settings');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeInTheDocument();
  });
});

describe('Start and hints send exactly {links} (spec 41 F5)', () => {
  const NAMES = ['config', 'egress', 'ingress', 'name'];

  it('Start sends {links: [{name, ingress, egress, config}]}: no settings, no worker, even from a legacy document', async () => {
    seedRaw(legacyDoc());
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    const body = JSON.parse(JSON.stringify(api.startEmulation.mock.calls[0][0])) as {
      links: Record<string, unknown>[];
    };
    expect(Object.keys(body)).toEqual(['links']);
    expect(body.links.map((l) => l.name)).toEqual(['a-b', 'b-a']);
    for (const l of body.links) {
      expect(
        Object.keys(l)
          .filter((k) => l[k] !== null && l[k] !== undefined)
          .sort(),
      ).toEqual(NAMES);
    }
    expect(JSON.stringify(body)).not.toMatch(/settings|worker|eal_extra|main_lcore|poll_interval/);
  });

  it('the hints request carries {links} only, from a legacy document too', async () => {
    seedRaw(legacyDoc());
    renderApp('/hints');
    await waitFor(() => expect(api.getHostHintsFor).toHaveBeenCalledTimes(1));
    const body = JSON.parse(JSON.stringify(api.getHostHintsFor.mock.calls[0][0])) as {
      links: Record<string, unknown>[];
    };
    expect(Object.keys(body)).toEqual(['links']);
    for (const l of body.links) {
      expect(
        Object.keys(l)
          .filter((k) => l[k] !== null && l[k] !== undefined)
          .sort(),
      ).toEqual(NAMES);
    }
    expect(JSON.stringify(body)).not.toMatch(/settings|worker/);
  });

  it('startRequestOf builds {links} with name, ingress, egress and config only', () => {
    const doc = parseStored(legacyDoc()).doc;
    const req = startRequestOf(doc);
    expect(Object.keys(req)).toEqual(['links']);
    expect(req.links).toHaveLength(2);
    for (const l of req.links) {
      expect(Object.keys(l).sort()).toEqual(NAMES);
    }
    expect(req.links[0]).toEqual({
      name: 'a-b',
      ingress: '0000:65:00.0',
      egress: '0000:65:00.1',
      config: fx.config(),
    });
  });
});

describe('the pending-restart marker compares links only (spec 41 F5)', () => {
  it('a running config that still carries settings and a worker does not differ from the local links', () => {
    const local = parseStored(JSON.stringify({ version: 1, links: [fx.link(), fx.linkRev()] })).doc;
    const config = fx.runningConfig([fx.link(), fx.linkRev()]);
    const noisy = {
      ...config,
      settings: { main_lcore: 4, eal_extra_args: ['--no-pci'], poll_interval_ms: 900 },
      links: config.links.map((l) => ({ ...l, worker: 7 })),
    };
    const emulation = fx.runningEmulation(noisy);
    const result = pendingRestart(local, emulation);
    expect(result.any).toBe(false);
    expect(result.links.size).toBe(0);
  });

  it('a different link still differs', () => {
    const changed: Link = { ...fx.link(), egress: 'net_null7' };
    const local = parseStored(JSON.stringify({ version: 1, links: [changed, fx.linkRev()] })).doc;
    const result = pendingRestart(local, fx.runningEmulation(fx.runningConfig([fx.link(), fx.linkRev()])));
    expect(result.any).toBe(true);
    expect([...result.links]).toEqual(['a-b']);
  });

  it('"Load running configuration" copies the links without a worker even from an old controller reply', async () => {
    const user = userEvent.setup();
    const config = fx.runningConfig([fx.link(), fx.linkRev()]);
    const noisy = {
      ...config,
      settings: LEGACY_SETTINGS,
      links: config.links.map((l) => ({ ...l, worker: 3 })),
    };
    api.getEmulation.mockResolvedValue(fx.runningEmulation(noisy));
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(storedDoc().links).toHaveLength(2));
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'version']);
    expect(hasMember(storedDoc().links, 'worker')).toBe(false);
  });
});

describe('a stored document with settings or worker (spec 41 F3)', () => {
  it('loads: no alert, the links are listed, and reading writes nothing', async () => {
    const raw = legacyDoc();
    seedRaw(raw);
    renderApp('/settings');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.getByText('b-a')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^reset/i })).not.toBeInTheDocument();
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(rawStore()).toBe(raw);
  });

  it.each([
    ['a settings member of the wrong shape (a string)', { settings: 'fast' }],
    ['a settings member that is null', { settings: null }],
    [
      'a settings member with a bad poll interval',
      { settings: { main_lcore: 'x', eal_extra_args: 5, poll_interval_ms: 'soon' } },
    ],
    ['no settings member at all', { settings: undefined }],
  ])('%s is no reason to refuse the document', async (_what, over) => {
    seedRaw(legacyDoc(over));
    renderApp('/settings');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it.each([
    ['a negative worker', -1],
    ['a fractional worker', 1.5],
    ['a worker that is a string', '3'],
    ['a worker that is null', null],
    ['worker 0', 0],
  ])('%s of a link is ignored, not an error', async (_what, worker) => {
    seedRaw(JSON.stringify({ version: 1, links: [{ ...fx.link(), worker }] }));
    renderApp('/settings');
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(isLocalConfig({ version: 1, links: [{ ...fx.link(), worker }] })).toBe(true);
  });

  it('parseStored drops both: the document is {version, links} and no link has a worker', () => {
    const { corrupt, doc } = parseStored(legacyDoc());
    expect(corrupt).toBe(false);
    expect(Object.keys(doc).sort()).toEqual(['links', 'version']);
    expect(doc.links.map((l) => l.name)).toEqual(['a-b', 'b-a']);
    expect(hasMember(doc.links, 'worker')).toBe(false);
  });

  it('the empty document is {version: 1, links: []}', () => {
    expect(emptyConfig()).toEqual({ version: 1, links: [] });
  });

  it('writeConfig stores version and links only', () => {
    const doc = parseStored(legacyDoc()).doc;
    writeConfig({ ...doc, settings: LEGACY_SETTINGS, links: legacyLinks() } as unknown as typeof doc);
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'version']);
    expect(hasMember(storedDoc().links, 'worker')).toBe(false);
  });

  it('Start works with it (Start is enabled)', async () => {
    seedRaw(legacyDoc());
    renderApp('/');
    await waitFor(() => expect(screen.getByRole('button', { name: /^start$/i })).toBeEnabled());
  });

  const expectClean = () => {
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'version']);
    expect(hasMember(storedDoc().links, 'worker')).toBe(false);
  };

  it('deleting a link drops settings and every worker, also from the links that stay', async () => {
    seedRaw(legacyDoc());
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Delete b-a' }));
    await user.click(screen.getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(storedDoc().links.map((l) => l.name)).toEqual(['a-b']));
    expectClean();
  });

  it('saving an edited link drops them', async () => {
    seedRaw(legacyDoc());
    const user = userEvent.setup();
    renderApp('/settings/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('a-b'));
    expect(screen.queryByLabelText(/worker|lcore/i)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expectClean();
    expect(storedDoc().links.map((l) => l.name)).toEqual(['a-b', 'b-a']);
  });

  it('saving a new link drops them', async () => {
    seedRaw(legacyVdevDoc());
    const user = userEvent.setup();
    renderApp('/settings/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    await user.type(screen.getByLabelText('Name'), 'extra');
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.0');
    await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expectClean();
    expect(storedDoc().links.map((l) => l.name)).toEqual(['a-b', 'b-a', 'extra']);
  });
});

describe('Duplicate, editor and inspect panel have no worker (spec 41 F4)', () => {
  it('Duplicate opens a copy without a worker field and saves it without a worker member', async () => {
    seedRaw(legacyVdevDoc());
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Duplicate a-b' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue('a-b_copy');
    expect(screen.queryByLabelText(/worker|lcore/i)).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.1');
    await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.0');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    const links = storedDoc().links;
    expect(links.map((l) => l.name)).toEqual(['a-b', 'b-a', 'a-b_copy']);
    expect(hasMember(links, 'worker')).toBe(false);
  });

  it('the new-link editor has no worker or core field, and no tooltip button for one', async () => {
    renderApp('/settings/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    expect(screen.queryByLabelText(/worker|lcore|core/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /About (worker|lcore)/i })).not.toBeInTheDocument();
    expect(screen.queryByText(/worker|lcore/i)).not.toBeInTheDocument();
  });

  it('the Links list (Settings page) has no worker or core column or text', async () => {
    seedRaw(legacyDoc());
    renderApp('/settings');
    const row = (await screen.findByText('a-b')).closest('tr');
    expect(must(row as HTMLElement | null, 'row of a-b').textContent).not.toMatch(/worker|lcore|core/i);
    expect(screen.queryByRole('columnheader', { name: /worker|lcore|core/i })).not.toBeInTheDocument();
  });

  it('the inspect panel of a link that carries a worker shows no worker, no lcore and no "auto"', async () => {
    seedRaw(legacyDoc());
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Inspect a-b' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).queryByText(/worker|lcore/i)).not.toBeInTheDocument();
    expect(dialog.textContent).not.toMatch(/\bauto\b/i);
    expect(within(dialog).getByText('Ingress')).toBeInTheDocument();
    expect(within(dialog).getByRole('link', { name: 'Edit a-b' })).toHaveAttribute('href', '/settings/1');
  });
});

describe('the frontend source has no settings and no worker left (spec 41 F6)', () => {
  const srcDir = resolve(__dirname, '..');
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
      const p = resolve(dir, e.name);
      if (e.isDirectory()) return e.name === 'test' || e.name === 'assets' ? [] : walk(p);
      return /\.(ts|tsx)$/.test(e.name) && !/\.test\.(ts|tsx)$/.test(e.name) ? [p] : [];
    });
  const files = walk(srcDir);
  const text = (name: string) => readFileSync(resolve(srcDir, name), 'utf8');

  it('finds the source files', () => {
    expect(files.length).toBeGreaterThan(10);
  });

  it('api/types.ts has no Settings type, no worker, and no settings member in the start request or the stored document', () => {
    const types = text('api/types.ts');
    expect(types).not.toMatch(/\binterface Settings\b/);
    expect(types).not.toMatch(/\bworker\b/i);
    expect(types).not.toMatch(/\bsettings\s*[?]?\s*:/);
    expect(types).not.toMatch(/eal_extra_args|poll_interval_ms/);
  });

  it.each(files.map((f) => [f.slice(srcDir.length + 1), f]))(
    '%s has no settings code and no worker field',
    (_name, file) => {
      const src = readFileSync(file, 'utf8');
      expect(src).not.toMatch(/eal_extra_args|poll_interval_ms|settingsOf|DEFAULT_SETTINGS/);
      expect(src).not.toMatch(/\bworker\b|Worker lcore/i);
    },
  );
});
