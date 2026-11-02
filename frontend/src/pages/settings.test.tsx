import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

// Labels: "Main lcore" (number, empty = null/automatic), "mlx5 devargs" (text), "Poll interval (ms)" (number).
// EAL extra arguments are a LIST (F5): a `<fieldset>` (role group) named "EAL extra arguments" with one text
// input per argument labelled "EAL argument 1", "EAL argument 2", ... (1-based row numbers), a button
// "Remove EAL argument N" per row and a button "Add EAL argument" (appends an empty row). Each row is exactly
// one element of eal_extra_args (never split on whitespace); empty rows are dropped when saving.
// Button "Save"; success -> role="status" text /saved/i; API error -> role="alert".
async function open() {
  const user = userEvent.setup();
  renderApp('/settings');
  await waitFor(() => expect(screen.getByLabelText('mlx5 devargs')).toHaveValue('txq_mpw_en=1'));
  return user;
}
const save = () => screen.getByRole('button', { name: /^save$/i });
const ealGroup = () => screen.getByRole('group', { name: 'EAL extra arguments' });
const ealRows = () => within(ealGroup()).queryAllByRole('textbox') as HTMLInputElement[];
const ealArg = (n: number) => screen.getByLabelText(`EAL argument ${n}`) as HTMLInputElement;
const addArg = () => screen.getByRole('button', { name: 'Add EAL argument' });

describe('Settings page', () => {
  it('loads the stored settings into the form', async () => {
    api.getSettings.mockResolvedValue(
      fx.settings({ main_lcore: 1, mlx5_devargs: 'txq_mpw_en=1', eal_extra_args: ['--no-huge', '-m', '256', '--no-pci'], poll_interval_ms: 250 }),
    );
    renderApp('/settings');
    await waitFor(() => expect(screen.getByLabelText('Main lcore')).toHaveValue(1));
    expect(screen.getByLabelText('mlx5 devargs')).toHaveValue('txq_mpw_en=1');
    await waitFor(() => expect(ealRows()).toHaveLength(4));
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '-m', '256', '--no-pci']);
    expect(ealArg(1)).toHaveValue('--no-huge');
    expect(ealArg(4)).toHaveValue('--no-pci');
    expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(250);
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
    await user.clear(screen.getByLabelText('mlx5 devargs'));
    await user.type(screen.getByLabelText('mlx5 devargs'), 'txq_mpw_en=1,rx_vec_en=1');
    await user.click(addArg());
    await user.type(ealArg(1), '--no-huge');
    await user.click(addArg());
    await user.type(ealArg(2), '-m');
    await user.click(addArg());
    await user.type(ealArg(3), '256');
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), '100');
    await user.click(save());
    expect(api.updateSettings).toHaveBeenCalledTimes(1);
    expect(api.updateSettings).toHaveBeenCalledWith({
      main_lcore: 2, mlx5_devargs: 'txq_mpw_en=1,rx_vec_en=1', eal_extra_args: ['--no-huge', '-m', '256'], poll_interval_ms: 100,
    });
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
  });

  it('clearing the main lcore sends null; empty EAL args send []', async () => {
    api.getSettings.mockResolvedValue(fx.settings({ main_lcore: 3, eal_extra_args: ['--no-pci'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(screen.getByLabelText('Main lcore')).toHaveValue(3));
    await user.clear(screen.getByLabelText('Main lcore'));
    await user.click(screen.getByRole('button', { name: 'Remove EAL argument 1' }));
    expect(ealRows()).toHaveLength(0);
    await user.click(save());
    const sent = api.updateSettings.mock.calls[0][0];
    expect(sent.main_lcore ?? null).toBeNull();
    expect(sent.eal_extra_args).toEqual([]);
  });

  it('shows the API validation error and no success message', async () => {
    api.updateSettings.mockRejectedValue(new ApiError(400, 'poll_interval_ms must be > 0'));
    const user = await open();
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.type(screen.getByLabelText('Poll interval (ms)'), '0');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('poll_interval_ms must be > 0');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Poll interval (ms)')).toHaveValue(0);
  });

  it('shows the API rejection of a forbidden devarg / EAL argument', async () => {
    api.updateSettings.mockRejectedValue(new ApiError(400, 'eal_extra_args: "--vdev" is not allowed'));
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--vdev');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('"--vdev" is not allowed');
  });

  it('does not call the API with an empty poll interval', async () => {
    const user = await open();
    await user.clear(screen.getByLabelText('Poll interval (ms)'));
    await user.click(save());
    expect(api.updateSettings).not.toHaveBeenCalled();
  });

  it('clears the success message again when the form is edited afterwards', async () => {
    const user = await open();
    await user.click(save());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    await user.type(screen.getByLabelText('mlx5 devargs'), ',rx_vec_en=1');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps an argument with spaces as ONE list element (no whitespace splitting)', async () => {
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--log-level=lib.eal:debug extra');
    await user.click(save());
    expect(api.updateSettings.mock.calls[0][0].eal_extra_args).toEqual(['--log-level=lib.eal:debug extra']);
  });

  it('typing a space-separated line into one row does not split it either', async () => {
    const user = await open();
    await user.click(addArg());
    await user.type(ealArg(1), '--no-huge -m 256');
    await user.click(save());
    expect(api.updateSettings.mock.calls[0][0].eal_extra_args).toEqual(['--no-huge -m 256']);
  });

  it('empty rows are dropped when saving', async () => {
    const user = await open();
    await user.click(addArg());
    await user.click(addArg());
    await user.click(addArg());
    await user.type(ealArg(2), '--no-pci');
    await user.click(save());
    expect(api.updateSettings.mock.calls[0][0].eal_extra_args).toEqual(['--no-pci']);
  });

  it('"Add EAL argument" appends an empty row at the end and keeps the other rows', async () => {
    api.getSettings.mockResolvedValue(fx.settings({ eal_extra_args: ['--no-huge', '--no-pci'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(2));
    await user.click(addArg());
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '--no-pci', '']);
  });

  it('removing a row removes exactly that argument and renumbers the rest', async () => {
    api.getSettings.mockResolvedValue(fx.settings({ eal_extra_args: ['--no-huge', '-m', '256'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(3));
    await user.click(screen.getByRole('button', { name: 'Remove EAL argument 2' }));
    expect(ealRows().map((r) => r.value)).toEqual(['--no-huge', '256']);
    expect(ealArg(2)).toHaveValue('256');
    await user.click(save());
    expect(api.updateSettings.mock.calls[0][0].eal_extra_args).toEqual(['--no-huge', '256']);
  });

  it('editing one row leaves the others untouched', async () => {
    api.getSettings.mockResolvedValue(fx.settings({ eal_extra_args: ['--no-huge', '-m', '256'] }));
    const user = userEvent.setup();
    renderApp('/settings');
    await waitFor(() => expect(ealRows()).toHaveLength(3));
    await user.clear(ealArg(3));
    await user.type(ealArg(3), '512');
    await user.click(save());
    expect(api.updateSettings.mock.calls[0][0].eal_extra_args).toEqual(['--no-huge', '-m', '512']);
  });

  it('changing the list clears the success message', async () => {
    const user = await open();
    await user.click(save());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    await user.click(addArg());
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('shows the API error if the settings cannot be loaded', async () => {
    api.getSettings.mockRejectedValue(new ApiError(500, 'state unreadable'));
    renderApp('/settings');
    expect(await screen.findByRole('alert')).toHaveTextContent('state unreadable');
  });
});
