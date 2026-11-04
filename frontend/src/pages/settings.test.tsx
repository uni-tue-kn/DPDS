import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { rawStore, renderApp, seedSettings, seedStore, storedDoc } from '../test/helpers';

// v22: the settings live in the local document (key dpds.config.v1, member `settings`) and nothing is requested from the controller.
// Without a stored document the form shows the defaults main lcore empty, no EAL arguments, poll interval 500 ms; viewing writes
// nothing; Save writes `settings` (links, presets and ids stay) and shows the status. Values are validated at Save: the poll
// interval must be an integer >= 1, the main lcore an integer or empty.
// Labels: "Main lcore" (number, empty = null/automatic), "Poll interval (ms)" (number). There is NO mlx5 devargs
// field (spec v17: the controller fixes them to PREOF's `txq_mpw_en=1`), and the page proposes no devargs.
// EAL extra arguments are a LIST (F5): a `<fieldset>` (role group) named "EAL extra arguments" with one text
// input per argument labelled "EAL argument 1", "EAL argument 2", ... (1-based row numbers), a button
// "Remove EAL argument N" per row and a button "Add EAL argument" (appends an empty row). Each row is exactly
// one element of eal_extra_args (never split on whitespace); empty rows are dropped when saving.
// Button "Save"; success -> role="status" text /saved/i; a refused value -> role="alert", nothing written.
async function open() {
  const user = userEvent.setup();
  renderApp('/settings');
  await waitFor(() => expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(500));
  return user;
}
const save = () => screen.getByRole('button', { name: /^save$/i });
const ealGroup = () => screen.getByRole('group', { name: 'EAL extra arguments' });
const ealRows = () => within(ealGroup()).queryAllByRole('textbox') as HTMLInputElement[];
const ealArg = (n: number) => screen.getByLabelText(`EAL argument ${n}`) as HTMLInputElement;
const addArg = () => screen.getByRole('button', { name: 'Add EAL argument' });

describe('Settings page', () => {
  it('loads the stored settings into the form', async () => {
    seedSettings(fx.settings({ main_lcore: 1, eal_extra_args: ['--no-huge', '-m', '256', '--no-pci'], poll_interval_ms: 250 }));
    renderApp('/settings');
    await waitFor(() => expect(screen.getByLabelText('Main lcore')).toHaveValue(1));
    await waitFor(() => expect(ealRows()).toHaveLength(4));
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '-m', '256', '--no-pci']);
    expect(ealArg(1)).toHaveValue('--no-huge');
    expect(ealArg(4)).toHaveValue('--no-pci');
    expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(250);
  });

  it('has no mlx5 devargs field and proposes no devargs (spec v17)', async () => {
    await open();
    expect(screen.queryByLabelText(/devargs/i)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/mlx5/i)).not.toBeInTheDocument();
    const page = document.body.textContent ?? '';
    for (const word of [/devargs/i, /mprq/i, /txq_mpw_en/, /rx_vec_en/, /txq_inline/, /rxq_cqe_comp_en/]) {
      expect(page).not.toMatch(word);
    }
    for (const box of screen.queryAllByRole('textbox') as HTMLInputElement[]) expect(box.value).not.toMatch(/_en=/);
    // exactly the remaining settings: main lcore, the EAL argument list, the poll interval
    expect(screen.getByLabelText('Main lcore')).toBeInTheDocument();
    expect(screen.getByLabelText('Poll interval (ms)')).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'EAL extra arguments' })).toBeInTheDocument();
  });

  it('sends the settings without mlx5_devargs even when saved unchanged', async () => {
    const user = await open();
    await user.click(save());
    expect(storedDoc().settings).toEqual({ main_lcore: null, eal_extra_args: [], poll_interval_ms: 500 });
    expect(Object.keys(storedDoc().settings).sort()).toEqual(['eal_extra_args', 'main_lcore', 'poll_interval_ms']);
  });

  it('shows an empty main lcore as empty (automatic)', async () => {
    await open();
    expect(screen.getByLabelText('Main lcore')).toHaveValue(null);
    expect(ealRows()).toHaveLength(0); // an empty list has no rows
    expect(addArg()).toBeEnabled();
  });

  it('saves edited values as typed (one list row per EAL argument)', async () => {
    const user = await open();
    await user.type(screen.getByLabelText('Main lcore'), '2');
    await user.click(addArg());
    await user.type(ealArg(1), '--no-huge');
    await user.click(addArg());
    await user.type(ealArg(2), '-m');
    await user.click(addArg());
    await user.type(ealArg(3), '256');
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), '100');
    await user.click(save());
    expect(storedDoc().settings).toEqual({ main_lcore: 2, eal_extra_args: ['--no-huge', '-m', '256'], poll_interval_ms: 100 });
    expect(storedDoc().settings).not.toHaveProperty('mlx5_devargs');
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
  });

  it('clearing the main lcore sends null; empty EAL args send []', async () => {
    seedSettings(fx.settings({ main_lcore: 3, eal_extra_args: ['--no-pci'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(screen.getByLabelText('Main lcore')).toHaveValue(3));
    await user.clear(screen.getByLabelText('Main lcore'));
    await user.click(screen.getByRole('button', { name: 'Remove EAL argument 1' }));
    expect(ealRows()).toHaveLength(0);
    await user.click(save());
    const sent = storedDoc().settings;
    expect(sent.main_lcore ?? null).toBeNull();
    expect(sent.eal_extra_args).toEqual([]);
  });

  it.each(['0', '-5', '1.5'])('a poll interval of %s is refused: alert naming it, no success message, nothing written, the typed value stays', async (v) => {
    const user = await open();
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), v);
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/poll interval/i);
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(Number(v));
    expect(rawStore()).toBeNull();
  });

  it('a main lcore that is not an integer is refused: alert naming it, nothing written', async () => {
    const user = await open();
    await user.type(screen.getByLabelText('Main lcore'), '1.5');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/main lcore/i);
    expect(rawStore()).toBeNull();
  });

  it('a refused value can be corrected and saved', async () => {
    const user = await open();
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), '0');
    await user.click(save());
    await screen.findByRole('alert');
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), '20');
    await user.click(save());
    expect(storedDoc().settings.poll_interval_ms).toBe(20);
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('EAL arguments are stored as typed; the controller judges them at start (nothing is refused here)', async () => {
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--vdev');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--vdev']);
  });

  it('opening the page shows the defaults and writes nothing; the first Save creates the document', async () => {
    const user = await open();
    expect(rawStore()).toBeNull();
    expect(screen.getByLabelText('Main lcore')).toHaveValue(null);
    await user.click(save());
    expect(storedDoc()).toMatchObject({ version: 1, links: [], presets: [] });
  });

  it('saving keeps the stored links (ids included) and presets', async () => {
    seedStore({ links: [fx.link(), fx.linkRev()], presets: [fx.preset({ id: 4 })], settings: fx.settings({ poll_interval_ms: 500 }) });
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(500));
    await user.type(screen.getByLabelText('Main lcore'), '3');
    await user.click(save());
    const doc = storedDoc();
    expect(doc.settings.main_lcore).toBe(3);
    expect(doc.links).toEqual([fx.link(), fx.linkRev()]);
    expect(doc.presets).toEqual([fx.preset({ id: 4 })]);
  });

  it('the page needs nothing from the controller: no emulation request', async () => {
    await open();
    expect(api.getEmulation).not.toHaveBeenCalled();
  });

  it('writes nothing with an empty poll interval', async () => {
    const user = await open();
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.click(save());
    expect(rawStore()).toBeNull();
    expect(await screen.findByRole('alert')).toHaveTextContent(/poll interval/i);
  });

  it('clears the success message again when the form is edited afterwards', async () => {
    const user = await open();
    await user.click(save());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    await user.type(screen.getByLabelText('Poll interval (ms)'), '0');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps an argument with spaces as ONE list element (no whitespace splitting)', async () => {
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--log-level=lib.eal:debug extra');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--log-level=lib.eal:debug extra']);
  });

  it('typing a space-separated line into one row does not split it either', async () => {
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--no-huge -m 256');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--no-huge -m 256']);
  });

  it('empty rows are dropped when saving', async () => {
    const user = await open();
    await user.click(addArg());
    await user.click(addArg());
    await user.click(addArg());
    await user.type(ealArg(2), '--no-pci');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--no-pci']);
  });

  it('"Add EAL argument" appends an empty row at the end and keeps the other rows', async () => {
    seedSettings(fx.settings({ eal_extra_args: ['--no-huge', '--no-pci'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(2));
    await user.click(addArg());
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '--no-pci', '']);
  });

  it('removing a row removes exactly that argument and renumbers the rest', async () => {
    seedSettings(fx.settings({ eal_extra_args: ['--no-huge', '-m', '256'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(3));
    await user.click(screen.getByRole('button', { name: 'Remove EAL argument 2' }));
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '256']);
    expect(ealArg(2)).toHaveValue('256');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--no-huge', '256']);
  });

  it('editing one row leaves the others untouched', async () => {
    seedSettings(fx.settings({ eal_extra_args: ['--no-huge', '-m', '256'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(3));
    await user.clear(ealArg(3));
    await user.type(ealArg(3), '512');
    await user.click(save());
    expect(storedDoc().settings.eal_extra_args).toEqual(['--no-huge', '-m', '512']);
  });

  it('changing the list clears the success message', async () => {
    const user = await open();
    await user.click(save());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    await user.click(addArg());
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });
});
