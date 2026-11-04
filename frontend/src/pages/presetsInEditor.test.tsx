import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import * as fx from '../test/fixtures';
import {
  addButton, queryAddButton, queryStage, renderApp, watchStoreWrites, seedStore, storedLinks, storedPresets, typeInto, typeRate,
  unitOf,
} from '../test/helpers';

// Contract: "Link editor additions" in the v8 block of src/test/helpers.tsx; v22: links and presets are in the local document
// (link 1 "a-b" is stored with the presets below), "Save as preset" appends a preset with id max+1, a name another preset has
// is refused inside the dialog, and a preset whose config lacks a stage removes that stage from the form when applied.
// Link 1 ("a-b", one directed link) = config(); preset "wan" = config3(), "other" = config2() (different everywhere).
const group = () => screen.getByRole('group', { name: /^Link configuration/ });
const field = (scope: HTMLElement, label: string) => within(scope).getByLabelText(label) as HTMLInputElement;
const applySelect = () => field(group(), 'Apply preset') as unknown as HTMLSelectElement;

function presets() {
  return [fx.preset({ id: 1, name: 'wan', config: fx.config3() }), fx.preset({ id: 2, name: 'other', description: null, config: fx.config2() })];
}

let writeCount: () => number = () => 0;
function watchWrites(): void {
  writeCount = watchStoreWrites();
}

async function openEdit(list = presets()): Promise<UserEvent> {
  seedStore({ links: [fx.link()], presets: list });
  const user = userEvent.setup();
  renderApp('/links/1');
  await screen.findByRole('heading', { level: 1, name: 'Edit link' });
  await waitFor(() => expect(field(group(), 'Mean delay')).toHaveValue(1));
  watchWrites();
  return user;
}

/** Compare a sent ConfigDto with the fixture, treating absent and null optional fields alike. */
function expectConfig(actual: ConfigDto, want: ConfigDto) {
  expect(actual.buffer_capacity).toBe(want.buffer_capacity);
  expect(actual.reordering).toBe(want.reordering);
  expect(actual.delayer ?? null).toEqual(want.delayer ?? null);
  expect(actual.limiter ?? null).toEqual(want.limiter ?? null);
  expect(actual.loss_model ?? null).toEqual(want.loss_model ?? null);
  expect(actual.seed ?? null).toBe(want.seed ?? null);
  expect(actual.histogram?.bins ?? null).toBe(want.histogram?.bins ?? null);
  expect(actual.histogram?.lo_ns ?? null).toBe(want.histogram?.lo_ns ?? null);
  expect(actual.histogram?.hi_ns ?? null).toBe(want.histogram?.hi_ns ?? null);
  expect(actual.delay_stats ?? true).toBe(want.delay_stats ?? true);
}

describe('Link editor: Apply preset', () => {
  it('the link form has exactly one "Apply preset" select listing the preset names (placeholder first)', async () => {
    await openEdit();
    expect(screen.getAllByLabelText('Apply preset')).toHaveLength(1);
    const opts = within(applySelect()).getAllByRole('option').map((o) => o.textContent);
    expect(opts).toHaveLength(3);
    expect(opts.slice(1)).toEqual(['wan', 'other']);
  });

  it('with no presets only the placeholder is offered', async () => {
    await openEdit([]);
    expect(within(applySelect()).getAllByRole('option')).toHaveLength(1);
  });

  it('a hairpin link has exactly one "Apply preset" select too', async () => {
    seedStore({ links: [fx.link(), fx.loopLink()], presets: presets() });
    renderApp('/links/2');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getAllByLabelText('Apply preset')).toHaveLength(1));
  });

  it('the editor works without any stored preset and does not request presets from the controller', async () => {
    seedStore({ links: [fx.link()] });
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(field(group(), 'Mean delay')).toHaveValue(1));
    expect(within(applySelect()).getAllByRole('option')).toHaveLength(1);
  });

  it('choosing a preset copies ALL its fields into the link\'s config form', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(), 'wan');
    const g = group();
    expect(field(g, 'Delay distribution')).toHaveValue('LogNormalDistribution');
    expect(field(g, 'Mean delay')).toHaveValue(7.777); // 7777 ns
    expect(unitOf(g, 'Mean delay')).toBe('\u00b5s');
    expect(field(g, 'Delay std')).toHaveValue(777);
    expect(unitOf(g, 'Delay std')).toBe('ns');
    expect(field(g, 'Activate correlation')).not.toBeChecked();
    expect(field(g, 'Half-life period')).toHaveValue(555);
    expect(unitOf(g, 'Half-life period')).toBe('ns');
    expect(field(g, 'Initialization rate')).toHaveValue(2.5); // v12: 2500000 pps
    expect(unitOf(g, 'Initialization rate')).toBe('Mpps');
    // v22: the link had no limiter; the preset has one, so the stage appears with the preset's values
    expect(queryStage('limiter')).toBeInTheDocument();
    expect(field(g, 'Limiter mode')).toHaveValue('Spacer');
    expect(field(g, 'Rate')).toHaveValue(5); // 5e9 bit/s
    expect(unitOf(g, 'Rate')).toBe('Gbit/s');
    expect(field(g, 'Loss model')).toHaveValue('Random');
    expect(field(g, 'Random loss probability')).toHaveValue(0.25);
    expect(field(g, 'Buffer capacity')).toHaveValue(321);
    expect(field(g, 'Reordering')).toBeChecked();
    expect(field(g, 'Seed')).toHaveValue(7);
    expect(field(g, 'Histogram bins')).toHaveValue(32);
    expect(field(g, 'Histogram lo')).toHaveValue(1); // 1000 ns
    expect(unitOf(g, 'Histogram lo')).toBe('\u00b5s');
    expect(field(g, 'Histogram hi')).toHaveValue(50); // 50000 ns
    expect(unitOf(g, 'Histogram hi')).toBe('\u00b5s');
    expect(field(g, 'Delay statistics')).toBeChecked();
  });

  it('copies a Gilbert-Elliot preset including the good-state flag (fields of the model appear)', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(), 'other'); // config2: GE p .01 r .2 k .9 h .3 bad start, delay_stats false
    const g = group();
    expect(field(g, 'Loss model')).toHaveValue('GilbertElliot');
    expect(field(g, 'Gilbert-Elliot p')).toHaveValue(0.01);
    expect(field(g, 'Gilbert-Elliot r')).toHaveValue(0.2);
    expect(field(g, 'Gilbert-Elliot k')).toHaveValue(0.9);
    expect(field(g, 'Gilbert-Elliot h')).toHaveValue(0.3);
    expect(field(g, 'Gilbert-Elliot starts in good state')).not.toBeChecked();
    expect(field(g, 'Delay statistics')).not.toBeChecked();
    expect(field(g, 'Delay distribution')).toHaveValue('GammaDistribution');
    expect(field(g, 'Initialization rate')).toHaveValue(8); // v12: 8 Mpps
  });

  it('saves exactly the applied config and leaves name, ingress, egress and worker as stored', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(), 'wan');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(storedLinks()).toHaveLength(1);
    const sent = storedLinks()[0];
    expect(sent.id).toBe(1);
    expectConfig(sent.config, fx.config3());
    expect(sent.name).toBe('a-b');
    expect(sent.ingress).toBe('0000:65:00.0');
    expect(sent.egress).toBe('0000:65:00.1');
    expect(sent.worker ?? null).toBeNull();
  });

  it('overwrites edits made before applying and does not touch name, ingress or egress', async () => {
    const user = await openEdit();
    await typeInto(user, group(), 'Mean delay', '123', 'ns');
    await user.selectOptions(applySelect(), 'wan');
    expect(field(group(), 'Mean delay')).toHaveValue(7.777);
    expect(screen.getByLabelText('Name')).toHaveValue('a-b');
    expect(screen.getByLabelText('Ingress')).toHaveValue('0000:65:00.0');
    expect(screen.getByLabelText('Egress')).toHaveValue('0000:65:00.1');
  });

  it('a form edited after applying is saved with the edit (the preset is a copy, not a reference)', async () => {
    const user = await openEdit();
    await user.selectOptions(applySelect(), 'wan');
    await typeInto(user, group(), 'Mean delay', '9999', 'ns');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = storedLinks()[0];
    expect(sent.config.delayer!.mean_ns).toBe(9999);
    expect(sent.config.delayer!.distribution).toBe('LogNormalDistribution');
    expect(sent.config.delayer!.initialization_rate_pps).toBe(2500000); // v12: copied from the preset
    expect(storedPresets().map((p) => p.id)).toEqual([1, 2]); // the presets are not touched
  });

  it('works on the "New link" page too', async () => {
    seedStore({ presets: presets() });
    const user = userEvent.setup();
    renderApp('/links/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(within(applySelect()).getAllByRole('option')).toHaveLength(3));
    expect(field(group(), 'Buffer capacity')).toHaveValue(100000);
    expect(queryStage('delayer')).not.toBeInTheDocument(); // v22: a new form starts without stages
    await user.selectOptions(applySelect(), 'wan');
    expect(field(group(), 'Buffer capacity')).toHaveValue(321);
    expect(field(group(), 'Initialization rate')).toHaveValue(2.5); // applied from the preset: its stages are added
    expect(queryStage('delayer')).toBeInTheDocument();
    expect(queryStage('limiter')).toBeInTheDocument();
    expect(queryStage('dropper')).toBeInTheDocument();
  });

  it('v22: applying a preset whose config lacks stages removes those stages from the form; the saved link has none of them', async () => {
    const user = await openEdit([fx.preset({ id: 1, name: 'bare', config: fx.configBare({ buffer_capacity: 77 }) }), ...presets().slice(1)]);
    await user.selectOptions(applySelect(), 'other'); // config2: all three stages
    expect(queryStage('limiter')).toBeInTheDocument();
    await user.selectOptions(applySelect(), 'bare');
    for (const s of ['Delayer', 'Limiter', 'Dropper']) expect(screen.queryByRole('group', { name: s })).not.toBeInTheDocument();
    for (const s of ['delayer', 'limiter', 'dropper'] as const) expect(queryAddButton(s)).toBeInTheDocument();
    expect(field(group(), 'Buffer capacity')).toHaveValue(77);
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expectConfig(storedLinks()[0].config, fx.configBare({ buffer_capacity: 77 }));
  });

  it('v22: applying a preset with a delayer to a form without one adds the delayer with the preset\'s values', async () => {
    const user = await openEdit();
    await user.click(screen.getByRole('button', { name: 'Remove delayer' }));
    expect(queryStage('delayer')).not.toBeInTheDocument();
    await user.selectOptions(applySelect(), 'other');
    expect(queryStage('delayer')).toBeInTheDocument();
    expect(field(group(), 'Mean delay')).toHaveValue(2.5);
    expect(queryAddButton('delayer')).not.toBeInTheDocument();
  });
});

describe('Link editor: Save as preset', () => {
  const saveAs = () => within(group()).getByRole('button', { name: 'Save as preset' });

  it('the link form has exactly one "Save as preset" button', async () => {
    await openEdit();
    expect(saveAs()).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Save as preset' })).toHaveLength(1);
  });

  it('opens a dialog with a name field; nothing is posted before confirming', async () => {
    const user = await openEdit();
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByLabelText('Preset name')).toHaveValue('');
    expect(within(dialog).getByLabelText('Description')).toBeInTheDocument();
    expect(writeCount()).toBe(0);
  });

  it('stores the config of the link (config() for link 1) with name, description and the next id', async () => {
    const user = await openEdit();
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'from-link');
    await user.type(within(dialog).getByLabelText('Description'), 'the link config');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(storedPresets()).toHaveLength(3);
    const sent = storedPresets()[2];
    expect(sent.id).toBe(3); // presets 1 and 2 exist: max+1
    expect(sent.name).toBe('from-link');
    expect(sent.description).toBe('the link config');
    expectConfig(sent.config, fx.config());
    expect(storedLinks()).toEqual([fx.link()]); // the link itself is not saved by this
    expect(storedPresets().slice(0, 2)).toEqual(presets());
  });

  it('posts the CURRENT (edited, unsaved) form values of the link', async () => {
    const user = await openEdit();
    await typeInto(user, group(), 'Mean delay', '4242', 'ns');
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'edited');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    const sent = storedPresets()[2];
    expect(sent.config.delayer!.mean_ns).toBe(4242);
    expect(sent.config.delayer!.initialization_rate_pps).toBe(12500000); // v12: the link's own value
    expect(storedLinks()[0].config.delayer!.mean_ns).toBe(1000); // the unsaved edit did not reach the link
    expect(sent.config.buffer_capacity).toBe(1000);
    expect(sent.description ?? null).toBeNull();
  });

  it('on success: dialog closes, a saved status shows and the preset is offered in "Apply preset"', async () => {
    const user = await openEdit([]);
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'fresh');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(await screen.findByRole('status')).toHaveTextContent(/saved/i);
    expect(within(applySelect()).getByRole('option', { name: 'fresh' })).toBeInTheDocument();
  });

  it('a name another preset already has is refused inside the dialog, which stays open; nothing is written', async () => {
    const user = await openEdit();
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'wan');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(/already exists/i);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(writeCount()).toBe(0);
  });

  it('an empty name sends nothing', async () => {
    const user = await openEdit();
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(writeCount()).toBe(0);
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('Cancel closes the dialog without a request', async () => {
    const user = await openEdit();
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'nope');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(writeCount()).toBe(0);
  });

  it('v22: a link form with no stage added is saved as a preset without stages', async () => {
    const user = await openEdit([]);
    for (const s of ['delayer', 'dropper'] as const) await user.click(screen.getByRole('button', { name: `Remove ${s}` }));
    await user.click(within(group()).getByRole('button', { name: 'Save as preset' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'bare');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(storedPresets()).toHaveLength(1);
    expectConfig(storedPresets()[0].config, fx.configBare({ buffer_capacity: 1000 }));
    expect(addButton('delayer')).toBeInTheDocument();
  });

  it('an invalid link config (seed above 2^53-1) is refused with an alert and no request', async () => {
    const user = await openEdit();
    await user.clear(field(group(), 'Seed'));
    await user.type(field(group(), 'Seed'), '9007199254740993');
    await user.click(saveAs());
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'bad-seed');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(await screen.findAllByRole('alert')).not.toHaveLength(0);
    expect(screen.getAllByRole('alert').some((a) => /seed/i.test(a.textContent ?? ''))).toBe(true);
    expect(writeCount()).toBe(0);
  });
});

describe('Link editor: initialization rate and presets (v12)', () => {
  it('"Save as preset" posts an edited initialization rate', async () => {
    const user = await openEdit();
    await typeRate(user, '3333333');
    await user.click(within(group()).getByRole('button', { name: 'Save as preset' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'rate');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(storedPresets()[2].config.delayer!.initialization_rate_pps).toBe(3333333);
  });

  it('"Save as preset" with an empty initialization rate is refused with an alert naming the field, no request', async () => {
    const user = await openEdit();
    await user.clear(field(group(), 'Initialization rate'));
    await user.click(within(group()).getByRole('button', { name: 'Save as preset' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), 'bad-rate');
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    expect(writeCount()).toBe(0);
    expect((await screen.findAllByRole('alert')).some((a) => /Initialization rate/.test(a.textContent ?? ''))).toBe(true);
  });

  it('applying a preset replaces a rate typed before, and the link is saved with the preset\'s rate', async () => {
    const user = await openEdit();
    await typeRate(user, '999');
    await user.selectOptions(applySelect(), 'other'); // config2: 8 Mpps
    expect(field(group(), 'Initialization rate')).toHaveValue(8);
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(storedLinks()[0].config.delayer!.initialization_rate_pps).toBe(8000000);
  });
});
