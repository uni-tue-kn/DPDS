import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState } from '../api/types';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { emptyDoc, rawStore, renderApp, seedStore, seedRaw, storedDoc } from '../test/helpers';
import type { StoredDoc } from '../test/helpers';

// Contract: "Export / import" in the v22 block of src/test/helpers.tsx: both happen in the browser on the local document
// (key dpds.config.v1), without any REST call. Export = the document as stored; import = a valid v1 document replaces it.
// jsdom has no URL.createObjectURL and no Blob.text(); the tests stub the former and the implementation must read
// the chosen file with FileReader. Downloads are observed through HTMLAnchorElement.prototype.click.
const BLOB_URL = 'blob:http://localhost/dpds-setup-test';

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
  await screen.findByLabelText('Poll interval (ms)');
  return user;
}

const exportBtn = () => screen.getByRole('button', { name: 'Export setup' });
const fileInput = () => screen.getByLabelText('Import setup file') as HTMLInputElement;
const jsonFile = (content: unknown, name = 'setup.json') =>
  new File([typeof content === 'string' ? content : JSON.stringify(content)], name, { type: 'application/json' });
const alertTexts = () =>
  screen
    .getAllByRole('alert')
    .map((a) => a.textContent ?? '')
    .join(' | ');

/** A document that differs from everything the app could produce by default. */
function doc(over: Partial<StoredDoc> = {}): StoredDoc {
  return {
    version: 1,
    links: [fx.link(), fx.linkRev({ id: 3 })],
    presets: [fx.preset({ id: 4 })],
    settings: fx.settings({ main_lcore: 2, eal_extra_args: ['--no-huge'], poll_interval_ms: 250 }),
    ...over,
  };
}

describe('Settings: export setup', () => {
  it('downloads the stored document as it is, as dpds-setup-<date>.json', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-11-04T12:00:00Z'));
    const stored = seedStore(doc());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(clicks[0].download).toBe('dpds-setup-2026-11-04.json');
    expect(clicks[0].href).toBe(BLOB_URL);
    expect(created).toHaveLength(1);
    expect(created[0].type).toMatch(/^application\/json/);
    expect(JSON.parse(await readBlob(created[0]))).toEqual(stored); // ids included
  });

  it('exports without any request to the controller', async () => {
    seedStore(doc());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(api.getEmulation).not.toHaveBeenCalled();
    // the sidebar's release check (spec18) may fetch GitHub; only the controller API is excluded here
    expect(vi.mocked(fetch).mock.calls.filter((c) => String(c[0]).includes('/api/'))).toEqual([]);
  });

  it('the file is the v1 document: version 1, links, presets and settings, no ports/configs arrays', async () => {
    seedStore(doc());
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    const parsed = JSON.parse(await readBlob(created[0])) as StoredDoc;
    expect(Object.keys(parsed).sort()).toEqual(['links', 'presets', 'settings', 'version']);
    expect(parsed.version).toBe(1);
    expect(parsed.links.length).toBeGreaterThan(0);
    for (const l of parsed.links) {
      expect(Object.keys(l)).toEqual(expect.arrayContaining(['id', 'name', 'ingress', 'egress', 'config']));
      expect(Object.keys(l)).not.toContain('ports');
    }
  });

  it('with nothing stored it exports the empty document (no links, no presets, default settings)', async () => {
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    expect(JSON.parse(await readBlob(created[0]))).toEqual(emptyDoc());
    expect(rawStore()).toBeNull(); // exporting does not create the stored document
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
});

describe('Settings: import setup', () => {
  async function upload(user: Awaited<ReturnType<typeof open>>, content: unknown = doc()) {
    await waitFor(() => expect(fileInput()).toBeEnabled());
    await user.upload(fileInput(), jsonFile(content));
  }
  const confirm = async () => within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' });

  it('choosing a file opens a confirm dialog warning that everything is replaced; nothing is written yet', async () => {
    const before = seedStore();
    const user = await open();
    await upload(user);
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent(/replace/i);
    expect(dialog).toHaveTextContent(/links/i);
    expect(dialog).toHaveTextContent(/settings/i);
    expect(dialog).toHaveTextContent(/presets/i);
    expect(within(dialog).getByRole('button', { name: 'Confirm import' })).toBeInTheDocument();
    expect(storedDoc()).toEqual(before);
  });

  it('Confirm replaces the stored document with the file, verbatim (ids kept), without any request', async () => {
    seedStore({ links: [fx.loopLink({ id: 9 })], presets: [], settings: fx.settings({ poll_interval_ms: 900 }) });
    const imported = doc();
    const user = await open();
    await upload(user, imported);
    await user.click(await confirm());
    await waitFor(() => expect(storedDoc()).toEqual(imported));
    // the sidebar's release check (spec18) may fetch GitHub; only the controller API is excluded here
    expect(vi.mocked(fetch).mock.calls.filter((c) => String(c[0]).includes('/api/'))).toEqual([]);
  });

  it('Cancel writes nothing and closes the dialog', async () => {
    const before = seedStore(doc({ presets: [] }));
    const user = await open();
    await upload(user, doc());
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(storedDoc()).toEqual(before);
  });

  it('success: status with the counts of the file, dialog closed, settings form shows the imported settings', async () => {
    const user = await open();
    await upload(user, doc());
    await user.click(await confirm());
    const status = await screen.findByText(/2 links/i);
    expect(status).toHaveTextContent(/1 presets?/i);
    expect(screen.getByRole('status')).toHaveTextContent(/imported/i);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(250));
    expect(screen.getByLabelText('Main lcore')).toHaveValue(2);
    expect(screen.getByLabelText('EAL argument 1')).toHaveValue('--no-huge');
  });

  it('an empty document (no links, no presets) is valid', async () => {
    seedStore(doc());
    const user = await open();
    await upload(user, emptyDoc());
    await user.click(await confirm());
    await waitFor(() => expect(storedDoc()).toEqual(emptyDoc()));
    expect(await screen.findByText(/0 links/i)).toBeInTheDocument();
  });

  it('a file that is not JSON: alert mentioning JSON, no dialog, nothing written', async () => {
    const before = seedStore(doc());
    const user = await open();
    await upload(user, 'this is { not json');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(storedDoc()).toEqual(before);
  });

  it('an empty file is refused like invalid JSON', async () => {
    const user = await open();
    await upload(user, '');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(rawStore()).toBeNull();
  });

  it.each([
    ['a JSON array', []],
    ['a version 2 document', { ...doc(), version: 2 }],
    ['a document without a version', (({ version: _v, ...rest }) => rest)(doc())],
    ['links that are not a list', { ...doc(), links: 'a-b' }],
    ['presets that are not a list', { ...doc(), presets: {} }],
    ['no settings', (({ settings: _s, ...rest }) => rest)(doc())],
    [
      'the old controller export format',
      { format: 'dpds-controller-export', version: 2, settings: fx.settings(), links: [], presets: [] },
    ],
  ])('%s is refused: alert naming the setup file, no dialog, nothing written', async (_name, content) => {
    const before = seedStore({ links: [fx.loopLink({ id: 9 })] });
    const user = await open();
    await upload(user, content);
    await waitFor(() => expect(screen.getAllByRole('alert').length).toBeGreaterThan(0));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(storedDoc()).toEqual(before);
  });

  it('does not write twice when Confirm is clicked twice quickly', async () => {
    const user = await open();
    await upload(user);
    const button = await confirm();
    await user.click(button);
    await user.click(button).catch(() => undefined); // may already be gone
    expect(storedDoc()).toEqual(doc());
  });

  it('importing over a corrupt stored value is possible and fixes it', async () => {
    seedRaw('{not json');
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(fileInput()).toBeEnabled());
    await user.upload(fileInput(), jsonFile(doc()));
    await user.click(await confirm());
    await waitFor(() => expect(storedDoc()).toEqual(doc()));
  });
});

describe('Settings: import is possible whatever the emulation does (nothing is stored on the controller)', () => {
  it.each(['stopped', 'starting', 'running', 'stopping', 'failed'] as EmulationState[])(
    '%s: input enabled, no stop hint',
    async (state) => {
      api.getEmulation.mockResolvedValue(fx.emulation(state));
      await open();
      expect(fileInput()).toBeEnabled();
      expect(screen.queryByTestId('import-hint')).not.toBeInTheDocument();
    },
  );

  it('a running emulation does not block the import; the page does not even ask for the state', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    const user = await open();
    await user.upload(fileInput(), jsonFile(doc()));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(storedDoc()).toEqual(doc()));
    expect(api.getEmulation).not.toHaveBeenCalled();
  });
});
