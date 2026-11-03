import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

// Contract: "Link editor additions" in the v8 block of src/test/helpers.tsx.
// Link 1 ("a-b"): direction 0 = config(), direction 1 = config2(); preset "wan" = config3() (different everywhere).
const group = (n: 0 | 1) => screen.getByRole('group', { name: new RegExp(`^Direction ${n}\\b`) });
const field = (scope: HTMLElement, label: string) => within(scope).getByLabelText(label) as HTMLInputElement;
const applySelect = (n: 0 | 1) => field(group(n), 'Apply preset') as unknown as HTMLSelectElement;

function presets() {
  return [fx.preset({ id: 1, name: 'wan', config: fx.config3() }), fx.preset({ id: 2, name: 'other', description: null, config: fx.config2() })];
}

async function openEdit(list = presets()): Promise<UserEvent> {
  api.getPresets.mockResolvedValue(list);
  const user = userEvent.setup();
  renderApp('/links/1');
  await screen.findByRole('heading', { level: 1, name: 'Edit link' });
  await waitFor(() => expect(field(group(0), 'Mean delay (ns)')).toHaveValue(1000));
  return user;
}

/** Compare a sent ConfigDto with the fixture, treating absent and null optional fields alike. */
function expectConfig(actual: ConfigDto, want: ConfigDto) {
  expect(actual.buffer_capacity).toBe(want.buffer_capacity);
  expect(actual.reordering).toBe(want.reordering);
  expect(actual.delayer).toMatchObject(want.delayer);
  expect(actual.limiter).toMatchObject(want.limiter);
  expect(actual.loss_model).toMatchObject(want.loss_model);
  expect(actual.seed ?? null).toBe(want.seed ?? null);
  expect(actual.histogram?.bins ?? null).toBe(want.histogram?.bins ?? null);
  expect(actual.histogram?.lo_ns ?? null).toBe(want.histogram?.lo_ns ?? null);
  expect(actual.histogram?.hi_ns ?? null).toBe(want.histogram?.hi_ns ?? null);
  expect(actual.delay_stats ?? true).toBe(want.delay_stats ?? true);
}

describe('Link editor: Apply preset', () => {
  it('every direction has an "Apply preset" select listing the preset names (placeholder first)', async () => {
    await openEdit();
    for (const d of [0, 1] as const) {
      const opts = within(applySelect(d)).getAllByRole('option').map((o) => o.textContent);
      expect(opts).toHaveLength(3);
      expect(opts.slice(1)).toEqual(['wan', 'other']);
    }
  });

  it('with no presets only the placeholder is offered', async () => {
    await openEdit([]);
    expect(within(applySelect(0)).getAllByRole('option')).toHaveLength(1);
  });

  it('a loopback link has exactly one "Apply preset" select', async () => {
    api.getPresets.mockResolvedValue(presets());
    api.getLink.mockImplementation(async () => fx.loopLink());
    renderApp('/links/2');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getAllByLabelText('Apply preset')).toHaveLength(1));
  });

  it('the editor still works when the presets cannot be loaded', async () => {
    api.getPresets.mockRejectedValue(new ApiError(500, 'presets unreadable'));
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(field(group(0), 'Mean delay (ns)')).toHaveValue(1000));
    expect(group(1)).toBeInTheDocument();
  });

  it('choosing a preset copies ALL its fields into that direction\'s form', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(0), 'wan');
    const g = group(0);
    expect(field(g, 'Delay distribution')).toHaveValue('LogNormalDistribution');
    expect(field(g, 'Mean delay (ns)')).toHaveValue(7777);
    expect(field(g, 'Delay std (ns)')).toHaveValue(777);
    expect(field(g, 'Activate correlation')).not.toBeChecked();
    expect(field(g, 'Half-life period (ns)')).toHaveValue(555);
    expect(field(g, 'Limiter active')).toBeChecked();
    expect(field(g, 'Limiter mode')).toHaveValue('Spacer');
    expect(field(g, 'Rate (bit/s)')).toHaveValue(5000000000);
    expect(field(g, 'Burst limit (bit)')).toHaveValue(64000);
    expect(field(g, 'Loss model')).toHaveValue('Random');
    expect(field(g, 'Random loss probability')).toHaveValue(0.25);
    expect(field(g, 'Buffer capacity')).toHaveValue(321);
    expect(field(g, 'Reordering')).toBeChecked();
    expect(field(g, 'Seed')).toHaveValue(7);
    expect(field(g, 'Histogram bins')).toHaveValue(32);
    expect(field(g, 'Histogram lo (ns)')).toHaveValue(1000);
    expect(field(g, 'Histogram hi (ns)')).toHaveValue(50000);
    expect(field(g, 'Delay statistics')).toBeChecked();
  });

  it('copies a Gilbert-Elliot preset including the good-state flag (fields of the model appear)', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(0), 'other'); // config2: GE p .01 r .2 k .9 h .3 bad start, delay_stats false
    const g = group(0);
    expect(field(g, 'Loss model')).toHaveValue('GilbertElliot');
    expect(field(g, 'Gilbert-Elliot p')).toHaveValue(0.01);
    expect(field(g, 'Gilbert-Elliot r')).toHaveValue(0.2);
    expect(field(g, 'Gilbert-Elliot k')).toHaveValue(0.9);
    expect(field(g, 'Gilbert-Elliot h')).toHaveValue(0.3);
    expect(field(g, 'Gilbert-Elliot starts in good state')).not.toBeChecked();
    expect(field(g, 'Delay statistics')).not.toBeChecked();
    expect(field(g, 'Delay distribution')).toHaveValue('GammaDistribution');
  });

  it('leaves the other direction untouched and saves exactly the applied config', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(0), 'wan');
    expect(field(group(1), 'Mean delay (ns)')).toHaveValue(2500); // still config2
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.updateLink).toHaveBeenCalledTimes(1);
    const sent = api.updateLink.mock.calls[0][1];
    expectConfig(sent.configs[0], fx.config3());
    expectConfig(sent.configs[1], fx.config2());
  });

  it('applying to direction 1 leaves direction 0 as stored', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(1), 'wan');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = api.updateLink.mock.calls[0][1];
    expectConfig(sent.configs[0], fx.config());
    expectConfig(sent.configs[1], fx.config3());
  });

  it('overwrites edits made before applying and does not touch name, ports or workers', async () => {
    const user = await openEdit();
    await user.clear(field(group(0), 'Mean delay (ns)'));
    await user.type(field(group(0), 'Mean delay (ns)'), '123');
    await user.selectOptions(applySelect(0), 'wan');
    expect(field(group(0), 'Mean delay (ns)')).toHaveValue(7777);
    expect(screen.getByLabelText('Name')).toHaveValue('a-b');
    expect(screen.getByLabelText('Port A')).toHaveValue('0000:65:00.0');
  });

  it('a form edited after applying is saved with the edit (the preset is a copy, not a reference)', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(0), 'wan');
    await user.clear(field(group(0), 'Mean delay (ns)'));
    await user.type(field(group(0), 'Mean delay (ns)'), '9999');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = api.updateLink.mock.calls[0][1];
    expect(sent.configs[0].delayer.mean_ns).toBe(9999);
    expect(sent.configs[0].delayer.distribution).toBe('LogNormalDistribution');
    expect(api.updatePreset).not.toHaveBeenCalled();
  });

  it('works on the "New link" page too', async () => {
    api.getPresets.mockResolvedValue(presets());
    const user = userEvent.setup();
    renderApp('/links/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(within(applySelect(1)).getAllByRole('option')).toHaveLength(3));
    await user.selectOptions(applySelect(1), 'wan');
    expect(field(group(1), 'Buffer capacity')).toHaveValue(321);
    expect(field(group(0), 'Buffer capacity')).toHaveValue(100000);
  });
});

describe('Link editor: Save as preset', () => {
  const saveAs = (n: 0 | 1) => within(group(n)).getByRole('button', { name: 'Save as preset' });

  it('each direction has a "Save as preset" button', async () => {
    await openEdit();
    expect(saveAs(0)).toBeInTheDocument();
    expect(saveAs(1)).toBeInTheDocument();
  });

  it('opens a dialog with a name field; nothing is posted before confirming', async () => {
    const user = await openEdit();
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByLabelText('Preset name')).toHaveValue('');
    expect(within(dialog).getByLabelText('Description')).toBeInTheDocument();
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('posts the config of THAT direction (direction 1 = config2)', async () => {
    const user = await openEdit();
    await user.click(saveAs(1));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'from-dir1');
    await user.type(within(dialog).getByLabelText('Description'), 'second direction');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(api.createPreset).toHaveBeenCalledTimes(1);
    const sent = api.createPreset.mock.calls[0][0];
    expect(sent.name).toBe('from-dir1');
    expect(sent.description).toBe('second direction');
    expectConfig(sent.config, fx.config2());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(api.updateLink).not.toHaveBeenCalled();
  });

  it('posts the CURRENT (edited, unsaved) form values of the direction', async () => {
    const user = await openEdit();
    await user.clear(field(group(0), 'Mean delay (ns)'));
    await user.type(field(group(0), 'Mean delay (ns)'), '4242');
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'edited');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    const sent = api.createPreset.mock.calls[0][0];
    expect(sent.config.delayer.mean_ns).toBe(4242);
    expect(sent.config.buffer_capacity).toBe(1000);
    expect(sent.description ?? null).toBeNull();
  });

  it('on success: dialog closes, a saved status shows and the preset is offered in "Apply preset"', async () => {
    api.getPresets.mockResolvedValueOnce([]).mockResolvedValue([fx.preset({ id: 77, name: 'fresh', config: fx.config() })]);
    const user = userEvent.setup();
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(field(group(0), 'Mean delay (ns)')).toHaveValue(1000));
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'fresh');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    expect(within(applySelect(0)).getByRole('option', { name: 'fresh' })).toBeInTheDocument();
    expect(within(applySelect(1)).getByRole('option', { name: 'fresh' })).toBeInTheDocument();
  });

  it('shows the API 409 inside the dialog and keeps it open', async () => {
    api.createPreset.mockRejectedValue(new ApiError(409, 'preset name "wan" already exists'));
    const user = await openEdit();
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'wan');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('preset name "wan" already exists');
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('an empty name sends nothing', async () => {
    const user = await openEdit();
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(api.createPreset).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('Cancel closes the dialog without a request', async () => {
    const user = await openEdit();
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'nope');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('an invalid direction config (seed above 2^53-1) is refused with an alert and no request', async () => {
    const user = await openEdit();
    await user.clear(field(group(0), 'Seed'));
    await user.type(field(group(0), 'Seed'), '9007199254740993');
    await user.click(saveAs(0));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'bad-seed');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(await screen.findAllByRole('alert')).not.toHaveLength(0);
    expect(screen.getAllByRole('alert').some((a) => /seed/i.test(a.textContent ?? ''))).toBe(true);
    expect(api.createPreset).not.toHaveBeenCalled();
  });
});
