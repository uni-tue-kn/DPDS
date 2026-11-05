import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState, Link } from '../api/types';
import { api } from '../test/mockApi';
import { EDGE_LINKS, INVALID_LINKS } from '../test/invalidLinks';
import * as fx from '../test/fixtures';
import { rawStore, renderApp, seedRaw, seedStore, storedDoc } from '../test/helpers';
import type { StoredDoc } from '../test/helpers';

// Contract: the "v27" and "spec41" blocks of src/test/helpers.tsx. Import and Export of the LINKS happen in the browser on the
// local document (key dpds.config.v1) without any REST call to import or export; since spec 41 they live in a toolbar at the top,
// right-aligned, of the Links page (the Settings page is gone). The file is { version: 1, links }: a `settings` member is ignored. jsdom has no URL.createObjectURL and no Blob.text(); the
// tests stub the former and the implementation must read the chosen file with FileReader. Downloads are observed through
// HTMLAnchorElement.prototype.click.
const BLOB_URL = 'blob:http://localhost/dpds-links-test';

let created: Blob[];
let clicks: { download: string; href: string }[];
let revoked: string[];

function readBlob(b: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result));
    r.onerror = () => reject(r.error);
    r.readAsText(b);
  });
}

beforeEach(() => {
  created = [];
  clicks = [];
  revoked = [];
  Object.defineProperty(URL, 'createObjectURL', {
    configurable: true,
    writable: true,
    value: vi.fn((b: Blob) => {
      created.push(b);
      return BLOB_URL;
    }),
  });
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true,
    writable: true,
    value: vi.fn((u: string) => {
      revoked.push(u);
    }),
  });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function click(this: HTMLAnchorElement) {
    clicks.push({ download: this.download, href: this.href });
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(URL, 'createObjectURL');
  Reflect.deleteProperty(URL, 'revokeObjectURL');
});

async function open() {
  const user = userEvent.setup();
  renderApp('/settings');
  await screen.findByRole('heading', { level: 1, name: 'Settings' });
  return user;
}

const importBtn = () => screen.getByRole('button', { name: 'Import' });
const exportBtn = () => screen.getByRole('button', { name: 'Export' });
const fileInput = () => screen.getByLabelText('Import links file') as HTMLInputElement;
const jsonFile = (content: unknown, name = 'links.json') =>
  new File([typeof content === 'string' ? content : JSON.stringify(content)], name, { type: 'application/json' });
const alertTexts = () =>
  screen
    .getAllByRole('alert')
    .map((a) => a.textContent ?? '')
    .join(' | ');

const LINKS = (): Link[] => [{ ...fx.link(), avg_frame_size_byte: 512 }, fx.linkRev({ id: 3 })];
/** A stored document with links (spec 41: nothing but `version` and `links`). */
const stored = (over: Partial<StoredDoc> = {}) => ({
  version: 1 as const,
  links: LINKS(),
  ...over,
});
/** The file of an export: version 1 and links, nothing else. */
const linksFile = (links: Link[] = LINKS()) => ({ version: 1, links });
const onlyKeys = (o: object) => Object.keys(o).sort();

describe('Links: the Import / Export toolbar (spec27, moved by spec41)', () => {
  it('shows buttons "Import" and "Export" and no Backup card, no "Export setup", no "Import setup file"', async () => {
    await open();
    expect(importBtn()).toBeInTheDocument();
    expect(exportBtn()).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: /backup/i })).not.toBeInTheDocument();
    expect(screen.queryByText(/backup/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /export setup/i })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Import setup file')).not.toBeInTheDocument();
  });

  it('Import carries the icon bi-cloud-arrow-down-fill, Export bi-cloud-arrow-up-fill (P4TG)', async () => {
    await open();
    expect(importBtn().querySelector('i.bi.bi-cloud-arrow-down-fill')).not.toBeNull();
    expect(exportBtn().querySelector('i.bi.bi-cloud-arrow-up-fill')).not.toBeNull();
  });

  it('the toolbar is at the TOP: both buttons come before the links table (or the empty-list text)', async () => {
    seedStore(stored());
    await open();
    const precedes = (a: Node, b: Node) => Boolean(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    const table = await screen.findByRole('table');
    expect(precedes(importBtn(), table)).toBe(true);
    expect(precedes(exportBtn(), table)).toBe(true);
  });

  it('the toolbar is also there, above the text, while no link is configured', async () => {
    await open();
    const empty = await screen.findByText('No links configured yet.');
    expect(importBtn().compareDocumentPosition(empty) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(exportBtn().compareDocumentPosition(empty) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('the order is [Import] [Export] and they sit in one right-aligned container', async () => {
    await open();
    expect(importBtn().compareDocumentPosition(exportBtn()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // right-aligned like P4TG's `<Col className="text-end">`: some ancestor that holds both buttons aligns to the end
    let box: HTMLElement | null = importBtn().parentElement;
    while (box && !box.contains(exportBtn())) box = box.parentElement;
    expect(box).not.toBeNull();
    const rightAligned = (el: HTMLElement | null) =>
      Array.from({ length: 3 }).some((_, up) => {
        let e = el;
        for (let i = 0; i < up && e; i++) e = e.parentElement;
        return e !== null && /\b(text-end|justify-content-end|ms-auto|float-end)\b/.test(e.className);
      });
    expect(rightAligned(box)).toBe(true);
  });

  it("Import uses the green button colour and Export the red one (btn-success / btn-danger; this theme's primary is red)", async () => {
    await open();
    expect(importBtn()).toHaveClass('btn-success');
    expect(exportBtn()).toHaveClass('btn-danger');
  });

  it('the file input is hidden, accepts .json and is named "Import links file"', async () => {
    await open();
    const input = fileInput();
    expect(input.type).toBe('file');
    expect(input.accept).toBe('.json');
    const hidden =
      input.hidden || input.style.display === 'none' || /\b(d-none|visually-hidden)\b/.test(input.className);
    expect(hidden).toBe(true);
  });

  it('clicking Import opens the file chooser (clicks the hidden input)', async () => {
    const user = await open();
    const spy = vi.spyOn(HTMLInputElement.prototype, 'click');
    await user.click(importBtn());
    expect(spy.mock.contexts.some((el) => (el as HTMLInputElement).type === 'file')).toBe(true);
  });

  it('the New link button is still on the page', async () => {
    await open();
    expect(screen.getByRole('link', { name: /new link/i })).toBeInTheDocument();
  });
});

describe('Links: export links', () => {
  it('downloads { version: 1, links } as dpds-links-<date>.json, links exactly as stored (ids, frame size)', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-11-04T12:00:00Z'));
    seedStore(stored());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(clicks[0].download).toBe('dpds-links-2026-11-04.json');
    expect(clicks[0].href).toBe(BLOB_URL);
    expect(created).toHaveLength(1);
    expect(created[0].type).toMatch(/^application\/json/);
    expect(JSON.parse(await readBlob(created[0]))).toEqual(linksFile());
  });

  it('is 2-space indented JSON', async () => {
    seedStore(stored());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    expect(await readBlob(created[0])).toBe(JSON.stringify(linksFile(), null, 2));
  });

  it('holds links only: no settings, no presets, even if the stored document carries presets and settings members', async () => {
    seedRaw(
      JSON.stringify({
        version: 1,
        links: LINKS(),
        presets: [{ id: 4, name: 'wan', config: fx.config3() }],
        settings: { main_lcore: 2, eal_extra_args: ['--no-huge'], poll_interval_ms: 250 },
      }),
    );
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    const parsed = JSON.parse(await readBlob(created[0])) as { version: number; links: Link[] };
    expect(onlyKeys(parsed)).toEqual(['links', 'version']);
    expect(parsed.version).toBe(1);
    for (const l of parsed.links) {
      expect(Object.keys(l)).toEqual(expect.arrayContaining(['id', 'name', 'ingress', 'egress', 'config']));
    }
  });

  it('exports without any request to the controller', async () => {
    seedStore(stored());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    // the sidebar's release check (spec18) may fetch GitHub; only the controller API is excluded here
    expect(vi.mocked(fetch).mock.calls.filter((c) => String(c[0]).includes('/api/'))).toEqual([]);
  });

  it('with nothing stored it exports { version: 1, links: [] } and does not create the stored document', async () => {
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    expect(JSON.parse(await readBlob(created[0]))).toEqual({ version: 1, links: [] });
    expect(rawStore()).toBeNull();
  });

  it('releases the object URL after the download', async () => {
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    await waitFor(() => expect(revoked).toEqual([BLOB_URL]));
  });

  it('is available while the emulation is running', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    await open();
    expect(exportBtn()).toBeEnabled();
  });

  it('exporting leaves the stored document untouched', async () => {
    seedStore(stored());
    const before = rawStore();
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(rawStore()).toBe(before);
  });
});

describe('Links: import links', () => {
  async function upload(user: Awaited<ReturnType<typeof open>>, content: unknown = linksFile()) {
    await user.upload(fileInput(), jsonFile(content));
  }
  const confirmButton = async () =>
    within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' });
  const newLinks = (): Link[] => [fx.loopLink({ id: 9 }), fx.link({ id: 12, name: 'x-y' })];

  it('choosing a file opens a confirm dialog "Importing will replace all existing links"; nothing is written yet', async () => {
    const before = seedStore(stored());
    const user = await open();
    await upload(user);
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent(/Importing will replace all existing links/i);
    expect(within(dialog).getByRole('button', { name: 'Confirm import' })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeInTheDocument();
    expect(storedDoc()).toEqual(before);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('Confirm replaces the links with the file (ids kept, verbatim)', async () => {
    seedStore({ links: [fx.loopLink({ id: 5 })] });
    const user = await open();
    await upload(user, linksFile(newLinks()));
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual(newLinks()));
    expect(onlyKeys(storedDoc())).toEqual(['links', 'version']);
    // the sidebar's release check (spec18) may fetch GitHub; only the controller API is excluded here
    expect(vi.mocked(fetch).mock.calls.filter((c) => String(c[0]).includes('/api/'))).toEqual([]);
  });

  it('a stored frame size of an imported link is kept', async () => {
    const user = await open();
    await upload(user, linksFile(LINKS()));
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual(LINKS()));
    expect(storedDoc().links[0].avg_frame_size_byte).toBe(512);
  });

  it('with nothing stored the import creates the document: the links of the file, nothing else', async () => {
    const user = await open();
    await upload(user, linksFile(newLinks()));
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual(newLinks()));
    expect(onlyKeys(storedDoc())).toEqual(['links', 'version']);
  });

  it('an extra `presets` member (an old setup file) is accepted and ignored: no presets member is written', async () => {
    seedStore({ links: [] });
    const user = await open();
    await upload(user, { ...linksFile(newLinks()), presets: [{ id: 4, name: 'wan', config: fx.config3() }] });
    const dialog = await screen.findByRole('dialog');
    expect(dialog).not.toHaveTextContent(/preset/i);
    await user.click(within(dialog).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc().links).toEqual(newLinks()));
    expect(onlyKeys(storedDoc())).toEqual(['links', 'version']);
  });

  it('an old full setup file (links, presets and settings) imports its links only: settings are not an error and not stored', async () => {
    seedStore(stored({ links: [fx.loopLink({ id: 5 })] }));
    const user = await open();
    await upload(user, {
      version: 1,
      links: newLinks(),
      presets: [{ id: 4, name: 'wan', config: fx.config3() }],
      settings: { main_lcore: 7, eal_extra_args: ['--no-huge'], poll_interval_ms: 999 },
    });
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual(newLinks()));
    expect(onlyKeys(storedDoc())).toEqual(['links', 'version']);
  });

  // spec 41 F3: a `worker` member of an imported link is ignored whatever its value, and never written back
  it.each([
    ['worker 5', 5],
    ['worker 0', 0],
    ['worker null', null],
    ['a negative worker', -1],
    ['a worker that is a string', '3'],
  ])('a link with %s is accepted and stored without a worker', async (_what, worker) => {
    const user = await open();
    await upload(user, linksFile([{ ...fx.link(), worker } as unknown as Link, fx.loopLink({ id: 9 })]));
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toHaveLength(2));
    expect(storedDoc().links[0]).toEqual(fx.link());
    expect(storedDoc().links.some((l) => 'worker' in l)).toBe(false);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('a settings member of any shape is ignored, not an error: nothing but version and links is stored', async () => {
    const user = await open();
    await upload(user, { ...linksFile(newLinks()), settings: 'fast' });
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual(newLinks()));
    expect(onlyKeys(storedDoc())).toEqual(['links', 'version']);
  });

  it('Export of a stored document that still carries settings and workers holds neither', async () => {
    seedRaw(
      JSON.stringify({
        version: 1,
        links: [{ ...fx.link(), worker: 5 }, fx.linkRev()],
        settings: { main_lcore: 2, eal_extra_args: [], poll_interval_ms: 250 },
      }),
    );
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    const parsed = JSON.parse(await readBlob(created[0])) as { links: object[] };
    expect(onlyKeys(parsed)).toEqual(['links', 'version']);
    expect(parsed.links.some((l) => 'worker' in l)).toBe(false);
  });

  it('Cancel writes nothing and closes the dialog', async () => {
    seedStore(stored());
    const before = rawStore();
    const user = await open();
    await upload(user, linksFile(newLinks()));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(rawStore()).toBe(before);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('success: status "Imported <n> links", dialog closed', async () => {
    const user = await open();
    await upload(user, linksFile(LINKS()));
    await user.click(await confirmButton());
    const status = await screen.findByRole('status');
    expect(status).toHaveTextContent('Imported 2 links');
    expect(status).not.toHaveTextContent(/preset/i);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('an empty links list is valid: it replaces all links and reports "Imported 0 links"', async () => {
    seedStore(stored());
    const user = await open();
    await upload(user, linksFile([]));
    await user.click(await confirmButton());
    await waitFor(() => expect(storedDoc().links).toEqual([]));
    expect(await screen.findByRole('status')).toHaveTextContent('Imported 0 links');
  });

  it('a file that is not JSON: inline alert mentioning JSON, no dialog, nothing written', async () => {
    seedStore(stored());
    const before = rawStore();
    const user = await open();
    await upload(user, 'this is { not json');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(rawStore()).toBe(before);
  });

  it('an empty file is refused like invalid JSON; the absent store stays absent', async () => {
    const user = await open();
    await upload(user, '');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(rawStore()).toBeNull();
  });

  const bad = (over: object) => ({ ...linksFile(), links: [{ ...fx.link(), ...over }] });
  it.each([
    ['a JSON array', []],
    ['JSON null', null],
    ['a version 2 file', { ...linksFile(), version: 2 }],
    ['a file without a version', { links: LINKS() }],
    ['a file without links', { version: 1 }],
    ['links that are not a list', { version: 1, links: 'a-b' }],
    ['a link that is not an object', { version: 1, links: ['a-b'] }],
    ['a link without a name', bad({ name: undefined })],
    ['a link whose name is not a string', bad({ name: 5 })],
    ['a link without an ingress port', bad({ ingress: undefined })],
    ['a link without an egress port', bad({ egress: 3 })],
    ['a link without a config', bad({ config: undefined })],
    ['a link whose config is not an object', bad({ config: 'x' })],
    ['a link without a usable id (0)', bad({ id: 0 })],
    ['a link whose id is not an integer', bad({ id: 1.5 })],
    ['a link whose id is a string', bad({ id: '1' })],
    ['the old controller export format', { format: 'dpds-controller-export', version: 2, links: [], presets: [] }],
  ])('%s is refused: inline alert, no dialog, nothing written', async (_name, content) => {
    seedStore(stored({ links: [fx.loopLink({ id: 9 })] }));
    const before = rawStore();
    const user = await open();
    await upload(user, content);
    await waitFor(() => expect(screen.getAllByRole('alert').length).toBeGreaterThan(0));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(rawStore()).toBe(before);
  });

  it('the refusal names the file', async () => {
    const user = await open();
    await user.upload(fileInput(), jsonFile({ version: 2, links: [] }, 'old-links.json'));
    await waitFor(() => expect(alertTexts()).toContain('old-links.json'));
  });

  it('a refusal is cleared by the next valid file', async () => {
    const user = await open();
    await upload(user, { version: 2, links: [] });
    await waitFor(() => expect(screen.getAllByRole('alert').length).toBeGreaterThan(0));
    await upload(user, linksFile());
    await screen.findByRole('dialog');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('the same file can be chosen again after Cancel', async () => {
    const user = await open();
    await upload(user);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await upload(user);
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
  });

  it('does not write twice when Confirm is clicked twice quickly', async () => {
    const writes = vi.spyOn(Storage.prototype, 'setItem');
    const user = await open();
    await upload(user);
    writes.mockClear();
    const button = await confirmButton();
    await user.click(button);
    await user.click(button).catch(() => undefined); // may already be gone
    expect(writes.mock.calls.filter((c) => c[0] === 'dpds.config.v1')).toHaveLength(1);
    expect(storedDoc().links).toEqual(LINKS());
  });

  it('nothing on the Links page (sidebar included) mentions presets', async () => {
    await open();
    expect(document.body).not.toHaveTextContent(/preset/i);
  });
});

describe('Links: import is possible whatever the emulation does (nothing is stored on the controller)', () => {
  it.each(['stopped', 'starting', 'running', 'stopping', 'failed'] as EmulationState[])(
    '%s: Import and the file input are enabled, no stop hint',
    async (state) => {
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      await open();
      expect(importBtn()).toBeEnabled();
      expect(fileInput()).toBeEnabled();
      expect(screen.queryByTestId('import-hint')).not.toBeInTheDocument();
    },
  );

  it('a running emulation does not block the import', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    const user = await open();
    await user.upload(fileInput(), jsonFile(linksFile()));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc().links).toEqual(LINKS()));
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).not.toHaveBeenCalled();
  });

  it('while running, an import only raises the pending-restart marker', async () => {
    const running = [fx.link(), fx.linkRev()];
    seedStore({ links: running });
    api.getEmulation.mockResolvedValue(fx.emulation('running', { config: fx.runningConfig(running) }));
    const user = await open();
    await user.upload(fileInput(), jsonFile(linksFile([fx.link({ name: 'other' })])));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc().links.map((l) => l.name)).toEqual(['other']));
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).not.toHaveBeenCalled();
  });
});

// ---- spec27b ------------------------------------------------------------------------------------------------------------------

// Item 1 (import side): a link is valid only if the link editor would accept it unchanged. Item 2: duplicate ids and names are
// refused, and an import error names the first offending link (its position, its name when it has one) and what is wrong.
/** The error text without the words "version 1" (they would match a position by accident). */
const positionText = (text: string) => text.replace(/version\s*1/gi, '');

describe('spec27b: import refuses links the editor would not accept', () => {
  async function importFile(content: unknown) {
    const user = await open();
    await user.upload(fileInput(), jsonFile(content));
    return user;
  }
  const refused = async (before: string | null) => {
    await waitFor(() => expect(screen.getAllByRole('alert').length).toBeGreaterThan(0));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(rawStore()).toBe(before);
  };

  it.each(INVALID_LINKS)(
    '$label is refused: inline alert saying what is wrong, no dialog, nothing written',
    async ({ link, problem }) => {
      seedStore(stored({ links: [fx.loopLink({ id: 9 })] }));
      const before = rawStore();
      await importFile({ version: 1, links: [link] });
      await refused(before);
      expect(alertTexts()).toMatch(problem);
    },
  );

  it('the error names the offending link: its position and its name', async () => {
    seedStore(stored());
    const before = rawStore();
    await importFile({
      version: 1,
      links: [fx.link({ id: 1, name: 'fine' }), { ...fx.link({ id: 2, name: 'broken' }), config: {} }],
    });
    await refused(before);
    const text = alertTexts();
    expect(text).toContain('broken');
    expect(positionText(text)).toMatch(/\b[12]\b/); // the position of the second link, counted from 0 or from 1
  });

  it('with several bad links it names the FIRST one only', async () => {
    seedStore(stored());
    const before = rawStore();
    await importFile({
      version: 1,
      links: [
        fx.link({ id: 1, name: 'fine' }),
        fx.link({ id: 2, name: 'first-bad', ingress: '' }),
        fx.link({ id: 3, name: 'second-bad', egress: '' }),
      ],
    });
    await refused(before);
    expect(alertTexts()).toContain('first-bad');
    expect(alertTexts()).not.toContain('second-bad');
    expect(alertTexts()).toMatch(/ingress/i);
  });

  it('a link that has no usable name is still located by its position', async () => {
    await importFile({ version: 1, links: [fx.link({ id: 1, name: 'fine' }), { ...fx.link({ id: 2 }), name: 7 }] });
    await refused(null);
    expect(positionText(alertTexts())).toMatch(/\b[12]\b/);
    expect(alertTexts()).toMatch(/name/i);
  });

  it('a link that is not even an object is located by its position too', async () => {
    await importFile({ version: 1, links: [fx.link({ id: 1, name: 'fine' }), 'oops'] });
    await refused(null);
    expect(positionText(alertTexts())).toMatch(/\b[12]\b/);
  });

  it('two links with the same id are refused and the error names the id', async () => {
    seedStore(stored());
    const before = rawStore();
    await importFile({
      version: 1,
      links: [fx.link({ id: 7, name: 'one' }), fx.loopLink({ id: 3 }), fx.link({ id: 7, name: 'two' })],
    });
    await refused(before);
    expect(alertTexts()).toMatch(/duplicate/i);
    expect(alertTexts()).toMatch(/\b7\b/);
  });

  it('two links with the same name are refused and the error names the name', async () => {
    seedStore(stored());
    const before = rawStore();
    await importFile({
      version: 1,
      links: [fx.link({ id: 1, name: 'same' }), fx.loopLink({ id: 2 }), fx.link({ id: 3, name: 'same' })],
    });
    await refused(before);
    expect(alertTexts()).toMatch(/duplicate/i);
    expect(alertTexts()).toContain('same');
  });

  it('nothing is written when the duplicate is the only fault, and a corrected file is accepted afterwards', async () => {
    const user = await importFile({
      version: 1,
      links: [fx.link({ id: 1, name: 'same' }), fx.link({ id: 2, name: 'same' })],
    });
    await refused(null);
    await user.upload(
      fileInput(),
      jsonFile({ version: 1, links: [fx.link({ id: 1, name: 'a' }), fx.link({ id: 2, name: 'b' })] }),
    );
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc().links.map((l) => l.name)).toEqual(['a', 'b']));
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it.each(EDGE_LINKS)('%s is accepted: the confirm dialog opens', async (_label, link) => {
    await importFile({ version: 1, links: [link] });
    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('an accepted file is stored verbatim, edge members included', async () => {
    const links = EDGE_LINKS.slice(0, 3).map(([, l], i) => ({ ...l, id: i + 1, name: `edge${i}` }));
    const user = await importFile({ version: 1, links });
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc().links).toHaveLength(3));
    expect(storedDoc().links).toEqual(JSON.parse(JSON.stringify(links)));
  });
});

// Item 5: Export on an unreadable store is the rescue path: the stored text, unchanged, under the same file name.
describe('spec27b: Export on a corrupt store downloads the stored text', () => {
  const brokenLink = { ...fx.link(), config: {} };
  const TEXTS: [string, string][] = [
    ['text that is not JSON', '{not json'],
    ['an empty string', ''],
    ['JSON of another shape', '[1, 2]'],
    ['a document of a newer version', JSON.stringify({ version: 7, links: [fx.link()] }, null, 4)],
    [
      'a document with an invalid link, with its own spacing',
      JSON.stringify({ version: 1, links: [brokenLink] }, null, 4),
    ],
  ];

  it.each(TEXTS)('%s: the blob is exactly the stored text', async (_label, raw) => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-11-04T12:00:00Z'));
    seedRaw(raw);
    const user = userEvent.setup();
    renderApp('/settings');
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(clicks[0].download).toBe('dpds-links-2026-11-04.json');
    expect(created).toHaveLength(1);
    expect(await readBlob(created[0])).toBe(raw);
  });

  it('Export is enabled, shows no error of its own, and the stored text stays untouched', async () => {
    const raw = '{not json';
    seedRaw(raw);
    const user = userEvent.setup();
    renderApp('/settings');
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(exportBtn()).toBeEnabled();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(rawStore()).toBe(raw);
    // the one alert on the page is the corrupt-config alert, not an export error
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('a readable store is still exported as { version: 1, links }, not as its raw text', async () => {
    seedRaw(JSON.stringify({ version: 1, links: LINKS(), settings: { poll_interval_ms: 250 }, future: 1 }, null, 4));
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    expect(await readBlob(created[0])).toBe(JSON.stringify(linksFile(), null, 2));
  });
});

// Item 7: the date in the file name is the local one.
describe('spec27b: the export file name carries the local date', () => {
  const original = process.env.TZ;
  afterEach(() => {
    if (original === undefined) Reflect.deleteProperty(process.env, 'TZ');
    else process.env.TZ = original;
  });

  it.each([
    // [time zone, the instant, the local date there] (the UTC date differs from the local one in each row)
    ['Pacific/Auckland', '2026-11-04T23:30:00Z', '2026-11-05'], // UTC+13: already the next day
    ['Pacific/Kiritimati', '2026-11-04T10:30:00Z', '2026-11-05'], // UTC+14
    ['America/Los_Angeles', '2026-11-05T03:00:00Z', '2026-11-04'], // UTC-8: still the previous day
    ['Pacific/Pago_Pago', '2026-11-05T05:00:00Z', '2026-11-04'], // UTC-11
  ])('in %s at %s the file is dpds-links-%s.json', async (zone, instant, localDate) => {
    process.env.TZ = zone;
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date(instant));
    const d = new Date();
    // guard: the test environment really switched zones (otherwise this test proves nothing)
    const local = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
    expect(local).toBe(localDate);
    expect(instant.slice(0, 10)).not.toBe(localDate);
    seedStore(stored());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(clicks[0].download).toBe(`dpds-links-${localDate}.json`);
  });

  it('a single-digit month and day are zero-padded (2026-01-05)', async () => {
    process.env.TZ = 'Europe/Berlin';
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-01-05T10:00:00Z'));
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(clicks[0].download).toBe('dpds-links-2026-01-05.json');
  });
});
