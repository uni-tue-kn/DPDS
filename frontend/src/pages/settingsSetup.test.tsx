import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { EmulationState, ExportDocument } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

// Contract: "Settings additions" in the v8 block of src/test/helpers.tsx.
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
    configurable: true, writable: true,
    value: vi.fn((b: Blob) => {
      created.push(b);
      return BLOB_URL;
    }),
  });
  Object.defineProperty(URL, 'revokeObjectURL', {
    configurable: true, writable: true,
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
  await waitFor(() => expect(screen.getByLabelText('mlx5 devargs')).toHaveValue('txq_mpw_en=1'));
  return user;
}

const exportBtn = () => screen.getByRole('button', { name: 'Export setup' });
const fileInput = () => screen.getByLabelText('Import setup file') as HTMLInputElement;
const jsonFile = (content: unknown, name = 'setup.json') =>
  new File([typeof content === 'string' ? content : JSON.stringify(content)], name, { type: 'application/json' });
const alertTexts = () => screen.getAllByRole('alert').map((a) => a.textContent ?? '').join(' | ');

describe('Settings: export setup', () => {
  it('downloads the controller\'s export document as dpds-setup-<date>.json', async () => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-11-03T12:00:00Z'));
    const doc = fx.exportDoc();
    api.exportSetup.mockResolvedValue(doc);
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    expect(api.exportSetup).toHaveBeenCalledTimes(1);
    expect(clicks[0].download).toBe('dpds-setup-2026-11-03.json');
    expect(clicks[0].href).toBe(BLOB_URL);
    expect(created).toHaveLength(1);
    expect(created[0].type).toMatch(/^application\/json/);
    expect(JSON.parse(await readBlob(created[0]))).toEqual(doc);
  });

  it('releases the object URL after the download', async () => {
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(clicks).toHaveLength(1));
    await waitFor(() => expect(revoked).toEqual([BLOB_URL]));
  });

  it('the downloaded file is valid JSON with the export format marker (not an HTML/error page)', async () => {
    const user = await open();
    await user.click(exportBtn());
    await waitFor(() => expect(created).toHaveLength(1));
    const parsed = JSON.parse(await readBlob(created[0])) as ExportDocument;
    expect(parsed.format).toBe('dpds-controller-export');
    expect(parsed.version).toBe(1);
  });

  it('shows the API error and downloads nothing when the export fails', async () => {
    api.exportSetup.mockRejectedValue(new ApiError(500, 'state unreadable'));
    const user = await open();
    await user.click(exportBtn());
    expect(await screen.findByRole('alert')).toHaveTextContent('state unreadable');
    expect(clicks).toHaveLength(0);
    expect(created).toHaveLength(0);
  });

  it('is available while the emulation is running', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    await open();
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    expect(exportBtn()).toBeEnabled();
  });
});

describe('Settings: import setup', () => {
  async function upload(user: Awaited<ReturnType<typeof open>>, content: unknown = fx.exportDoc()) {
    await waitFor(() => expect(fileInput()).toBeEnabled());
    await user.upload(fileInput(), jsonFile(content));
  }

  it('choosing a file opens a confirm dialog warning that everything is replaced; nothing is posted yet', async () => {
    const user = await open();
    await upload(user);
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent(/replace/i);
    expect(dialog).toHaveTextContent(/links/i);
    expect(dialog).toHaveTextContent(/settings/i);
    expect(dialog).toHaveTextContent(/presets/i);
    expect(within(dialog).getByRole('button', { name: 'Confirm import' })).toBeInTheDocument();
    expect(api.importSetup).not.toHaveBeenCalled();
  });

  it('Confirm posts the parsed document exactly once', async () => {
    const doc = fx.exportDoc();
    const user = await open();
    await upload(user, doc);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    expect(api.importSetup).toHaveBeenCalledTimes(1);
    expect(api.importSetup).toHaveBeenCalledWith(doc);
  });

  it('Cancel posts nothing and closes the dialog', async () => {
    const user = await open();
    await upload(user);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.importSetup).not.toHaveBeenCalled();
  });

  it('success: status with the stored counts, dialog closed, settings form reloaded from the controller', async () => {
    api.importSetup.mockResolvedValue({ links: 3, presets: 2 });
    api.getSettings.mockResolvedValueOnce(fx.settings()).mockResolvedValue(fx.settings({ mlx5_devargs: 'imported=1' }));
    const user = await open();
    await upload(user);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    const status = await screen.findByText(/3 links/i);
    expect(status).toHaveTextContent(/2 presets/i);
    expect(screen.getByRole('status')).toHaveTextContent(/imported/i);
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByLabelText('mlx5 devargs')).toHaveValue('imported=1'));
  });

  it('shows the API 400 message (validation of the document)', async () => {
    api.importSetup.mockRejectedValue(new ApiError(400, 'links[0].configs[1].delayer.std_ns must be >= 0'));
    const user = await open();
    await upload(user);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(alertTexts()).toContain('links[0].configs[1].delayer.std_ns must be >= 0'));
    expect(screen.queryByText(/imported/i)).not.toBeInTheDocument();
  });

  it('shows the API 409 message (emulation not stopped)', async () => {
    api.importSetup.mockRejectedValue(new ApiError(409, 'stop the emulation before importing'));
    const user = await open();
    await upload(user);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    await waitFor(() => expect(alertTexts()).toContain('stop the emulation before importing'));
  });

  it('a file that is not JSON: alert mentioning JSON, no dialog, no request', async () => {
    const user = await open();
    await upload(user, 'this is { not json');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(api.importSetup).not.toHaveBeenCalled();
  });

  it('an empty file is refused like invalid JSON', async () => {
    const user = await open();
    await upload(user, '');
    await waitFor(() => expect(alertTexts()).toMatch(/json/i));
    expect(api.importSetup).not.toHaveBeenCalled();
  });

  it('the document is posted untouched, unknown keys included (the controller decides)', async () => {
    const doc = { ...fx.exportDoc(), surprise: { a: 1 } } as unknown as ExportDocument;
    const user = await open();
    await upload(user, doc);
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
    expect(api.importSetup).toHaveBeenCalledWith(doc);
  });

  it('does not double-post when Confirm is clicked twice quickly', async () => {
    let release!: () => void;
    api.importSetup.mockImplementation(() => new Promise((res) => {
      release = () => res({ links: 1, presets: 1 });
    }));
    const user = await open();
    await upload(user);
    const confirm = within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' });
    await user.click(confirm);
    await user.click(confirm).catch(() => undefined); // may already be gone/disabled
    expect(api.importSetup).toHaveBeenCalledTimes(1);
    release();
  });
});

describe('Settings: import is only possible while stopped', () => {
  it('stopped: file input enabled, no hint', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('stopped'));
    await open();
    await waitFor(() => expect(fileInput()).toBeEnabled());
    expect(screen.queryByTestId('import-hint')).not.toBeInTheDocument();
  });

  it.each(['starting', 'running', 'stopping', 'failed'] as EmulationState[])('%s: input disabled and a stop hint shown', async (state) => {
    api.getEmulation.mockResolvedValue(fx.emulation(state));
    await open();
    await waitFor(() => expect(fileInput()).toBeDisabled());
    expect(screen.getByTestId('import-hint')).toHaveTextContent(/stop/i);
  });

  it('a disabled input never posts', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    const user = await open();
    await waitFor(() => expect(fileInput()).toBeDisabled());
    await user.upload(fileInput(), jsonFile(fx.exportDoc()));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(api.importSetup).not.toHaveBeenCalled();
  });
});
