import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto, Preset, PresetInput } from '../api/types';
import * as fx from '../test/fixtures';
import {
  addStages,
  rawStore,
  renderApp,
  watchStoreWrites,
  seedPresets,
  seedStore,
  storedDoc,
  storedPresets,
  stageOf,
  TEST_RATE,
  typeInto,
  typeRate,
  unitOf,
} from '../test/helpers';
import { must } from '../test/must';

// Contract: "Presets page" in the v8 block and the v22 block (local document, optional stages) of src/test/helpers.tsx.
// v22: presets live in the local document; new presets are appended with id max+1; `openNew` opens the form with delayer,
// limiter and dropper added (their fields are what most tests need), `openNew(false)` is the bare form.
const DISTRIBUTIONS = [
  'DeterministicDistribution',
  'LogNormalDistribution',
  'NormalDistribution',
  'GammaDistribution',
  'UniformDistribution',
];

function twoPresets() {
  return [
    fx.preset({ id: 1, name: 'wan-100ms', description: 'Typical WAN', config: fx.config2() }),
    fx.preset({ id: 2, name: 'lossy', description: null, config: fx.config3() }),
  ];
}

const lbl = (name: string) => screen.getByLabelText(name) as HTMLInputElement;
const save = () => screen.getByRole('button', { name: /^save$/i });

function expectValidConfig(c: ConfigDto) {
  expect(Number.isInteger(c.buffer_capacity)).toBe(true);
  expect(typeof c.reordering).toBe('boolean');
  expect(DISTRIBUTIONS).toContain(must(c.delayer).distribution);
  for (const v of [
    must(c.delayer).mean_ns,
    must(c.delayer).std_ns,
    must(c.delayer).half_life_period_ns,
    must(c.limiter).rate_bps,
  ]) {
    expect(Number.isFinite(v)).toBe(true);
  }
  expect(typeof must(c.delayer).initialization_rate_pps).toBe('number'); // v12: required, finite >= 1
  expect(Number.isFinite(must(c.delayer).initialization_rate_pps)).toBe(true);
  expect(must(c.delayer).initialization_rate_pps).toBeGreaterThanOrEqual(1);
  expect(['Spacer', 'TokenBucketFilter']).toContain(must(c.limiter).mode);
  expect(Object.keys(must(c.limiter)).sort()).toEqual(['mode', 'rate_bps']);
  expect(['Random', 'GilbertElliot']).toContain(must(c.loss_model).type);
  expect(typeof c.delay_stats).toBe('boolean');
}

describe('Presets: sidebar entry', () => {
  it('has a "Presets" link in the main navigation pointing at /presets', () => {
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(within(nav).getByRole('link', { name: 'Presets' })).toHaveAttribute('href', '/presets');
  });

  it('comes after the five pre-existing links (their order stays pinned by layout.test)', () => {
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    const names = within(nav)
      .getAllByRole('link')
      .map((a) => a.getAttribute('aria-label') ?? a.textContent?.trim());
    expect(names.indexOf('Presets')).toBeGreaterThanOrEqual(5);
  });

  it('opens the page with an h1 "Presets" and marks the link current', async () => {
    const user = userEvent.setup();
    renderApp('/');
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    await user.click(within(nav).getByRole('link', { name: 'Presets' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Presets' })).toBeInTheDocument();
    expect(within(nav).getByRole('link', { name: 'Presets' })).toHaveAttribute('aria-current', 'page');
    expect(within(nav).getByRole('link', { name: 'Links' })).not.toHaveAttribute('aria-current', 'page');
  });

  it('direct route /presets renders the page', async () => {
    renderApp('/presets');
    expect(await screen.findByRole('heading', { level: 1, name: 'Presets' })).toBeInTheDocument();
  });
});

describe('Presets: list', () => {
  it('lists name and description of every preset', async () => {
    seedPresets(twoPresets());
    renderApp('/presets');
    expect(await screen.findByText('wan-100ms')).toBeInTheDocument();
    expect(screen.getByText('Typical WAN')).toBeInTheDocument();
    expect(screen.getByText('lossy')).toBeInTheDocument();
    expect(screen.getAllByRole('row')).toHaveLength(3); // header + 2
  });

  it('empty list -> "no presets" text', async () => {
    renderApp('/presets');
    expect(await screen.findByText(/no presets/i)).toBeInTheDocument();
  });

  it('has a "New preset" link to /presets/new and Edit links by id', async () => {
    seedPresets(twoPresets());
    renderApp('/presets');
    expect(await screen.findByRole('link', { name: 'New preset' })).toHaveAttribute('href', '/presets/new');
    expect(screen.getByRole('link', { name: 'Edit wan-100ms' })).toHaveAttribute('href', '/presets/1');
    expect(screen.getByRole('link', { name: 'Edit lossy' })).toHaveAttribute('href', '/presets/2');
  });

  it('reads only the local document: nothing stored shows the empty state and creates no document', async () => {
    renderApp('/presets');
    expect(await screen.findByText(/no presets/i)).toBeInTheDocument();
    expect(rawStore()).toBeNull();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});

describe('Presets: delete', () => {
  it('asks for confirmation; Cancel does not delete', async () => {
    const user = userEvent.setup();
    seedPresets(twoPresets());
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('wan-100ms');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(storedPresets().map((p) => p.id)).toEqual([1, 2]);
  });

  it('Confirm deletes by id from the local document; the list updates; links and settings stay', async () => {
    const user = userEvent.setup();
    seedStore({ presets: twoPresets(), links: [fx.link()], settings: fx.settings({ main_lcore: 3 }) });
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(screen.queryByText('wan-100ms')).not.toBeInTheDocument());
    expect(screen.getByText('lossy')).toBeInTheDocument();
    const doc = storedDoc();
    expect(doc.presets.map((p) => p.id)).toEqual([2]);
    expect(doc.links).toEqual([fx.link()]);
    expect(doc.settings.main_lcore).toBe(3);
  });

  it('deleting a preset leaves the links that used its config as they are', async () => {
    const user = userEvent.setup();
    seedStore({ presets: twoPresets(), links: [fx.link({ config: fx.config2() })] });
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(screen.queryByText('wan-100ms')).not.toBeInTheDocument());
    expect(storedDoc().links[0].config).toEqual(fx.config2());
  });
});

let seeded = 0;
let writeCount: () => number = () => 0;
function watchWrites(): void {
  writeCount = watchStoreWrites();
}
const expectNoWrite = () => expect(writeCount()).toBe(0);

async function openNew(stages = true, others: Preset[] = []): Promise<UserEvent> {
  if (others.length > 0) seedPresets(others);
  seeded = others.length;
  const user = userEvent.setup();
  renderApp('/presets/new');
  await screen.findByRole('heading', { level: 1, name: 'New preset' });
  if (stages) await addStages(user);
  watchWrites();
  return user;
}

async function fillName(user: UserEvent, name: string) {
  await user.clear(lbl('Name'));
  await user.type(lbl('Name'), name);
}

/** Name plus the initialization rate (a new form starts without one), ready to save. */
async function fillValid(user: UserEvent, name: string) {
  await fillName(user, name);
  if (screen.queryByLabelText('Initialization rate')) await typeRate(user);
}

/** The preset the form created: one more than before, appended, with id max+1. */
function lastCreate(): PresetInput {
  const presets = storedPresets();
  expect(presets).toHaveLength(seeded + 1);
  expect(presets[seeded].id).toBe(Math.max(0, ...presets.slice(0, seeded).map((p) => p.id)) + 1);
  return presets[seeded];
}

describe('Presets: create', () => {
  it('shows name, description and exactly one direction config form with defaults', async () => {
    await openNew();
    expect(lbl('Name')).toHaveValue('');
    expect(lbl('Description')).toHaveValue('');
    expect(screen.getAllByLabelText('Mean delay')).toHaveLength(1);
    expect(screen.getAllByLabelText('Delay distribution')).toHaveLength(1);
    expect(screen.queryByLabelText('Limiter active')).not.toBeInTheDocument(); // v22: no switch
    expect(screen.queryByLabelText('Port A')).not.toBeInTheDocument(); // no link fields here
  });

  it('creates a preset with a valid default config and navigates to the list', async () => {
    const user = await openNew();
    await fillValid(user, 'my-preset');
    await user.type(lbl('Description'), 'my description');
    await user.click(save());
    const sent = lastCreate();
    expect(sent.name).toBe('my-preset');
    expect(sent.description).toBe('my description');
    expectValidConfig(sent.config);
    expect(sent.config.delay_stats).toBe(true);
    expect(sent.config.seed ?? null).toBeNull();
    expect(await screen.findByRole('heading', { level: 1, name: 'Presets' })).toBeInTheDocument();
  });

  it('sends edited config fields as typed', async () => {
    const user = await openNew();
    await fillValid(user, 'edited');
    await user.selectOptions(lbl('Delay distribution'), 'GammaDistribution');
    await typeInto(user, document.body, 'Mean delay', '2.5', '\u00b5s');
    await typeInto(user, document.body, 'Delay std', '300', 'ns');
    await user.selectOptions(lbl('Limiter mode'), 'TokenBucketFilter');
    await user.click(lbl('Reordering'));
    await user.click(save());
    const c = lastCreate().config;
    expect(c.delayer).toMatchObject({ distribution: 'GammaDistribution', mean_ns: 2500, std_ns: 300 });
    expect(c.limiter).toEqual({ mode: 'TokenBucketFilter', rate_bps: 1e10 });
    expect(c.reordering).toBe(true);
  });

  it('an empty description is sent as null or left out', async () => {
    const user = await openNew();
    await fillValid(user, 'nodesc');
    await user.click(save());
    expect(lastCreate().description ?? null).toBeNull();
  });

  it('refuses an empty name: alert, no request', async () => {
    const user = await openNew();
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expectNoWrite();
  });

  it('refuses a whitespace-only name', async () => {
    const user = await openNew();
    await fillName(user, '   ');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expectNoWrite();
  });

  it('refuses a seed above 2^53-1 like the link editor (alert naming the seed, no request)', async () => {
    const user = await openNew();
    await fillValid(user, 'big-seed');
    await user.type(lbl('Seed'), '9007199254740993');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/seed/i);
    expectNoWrite();
  });

  it('a name another preset already has is refused: alert "already exists", nothing written, the form and its values stay', async () => {
    const user = await openNew(true, twoPresets());
    await fillValid(user, 'lossy');
    await user.type(lbl('Description'), 'keep me');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/already exists/i);
    expectNoWrite();
    expect(screen.getByRole('heading', { level: 1, name: 'New preset' })).toBeInTheDocument();
    expect(lbl('Name')).toHaveValue('lossy');
    expect(lbl('Description')).toHaveValue('keep me');
    await fillName(user, 'lossy-2');
    await user.click(save());
    expect(lastCreate().name).toBe('lossy-2');
  });

  it('the first preset of an empty store gets id 1 and creates the document; opening the form wrote nothing', async () => {
    const user = await openNew();
    expect(rawStore()).toBeNull();
    await fillValid(user, 'first');
    await user.click(save());
    expect(storedDoc()).toMatchObject({ version: 1, links: [] });
    expect(storedPresets().map((p) => [p.id, p.name])).toEqual([[1, 'first']]);
  });

  it('a new preset next to presets 1 and 5 gets id 6; the others, the links and the settings are unchanged', async () => {
    const stored = [fx.preset({ id: 1, name: 'a' }), fx.preset({ id: 5, name: 'b' })];
    seedStore({ presets: stored, links: [fx.link()], settings: fx.settings({ poll_interval_ms: 250 }) });
    const user = await openNew(true);
    seeded = 2;
    await fillValid(user, 'c');
    await user.click(save());
    const doc = storedDoc();
    expect(doc.presets.map((p) => p.id)).toEqual([1, 5, 6]);
    expect(doc.presets.slice(0, 2)).toEqual(stored);
    expect(doc.links).toEqual([fx.link()]);
    expect(doc.settings.poll_interval_ms).toBe(250);
  });

  it('a refused value is reported without writing, and the corrected form saves', async () => {
    const user = await openNew();
    await fillValid(user, 'bad');
    await user.clear(lbl('Delay std'));
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Delay std/);
    expectNoWrite();
    await typeInto(user, document.body, 'Delay std', '5', 'ns');
    await user.click(save());
    expect(must(lastCreate().config.delayer).std_ns).toBe(5);
  });

  it('a bare form (no stage added) saves a preset with Buffer and Statistics only', async () => {
    const user = await openNew(false);
    await fillValid(user, 'bare');
    await user.click(save());
    const c = lastCreate().config;
    for (const k of ['delayer', 'limiter', 'loss_model'] as const) expect(stageOf(c, k)).toBeNull();
    expect(c.buffer_capacity).toBe(100000);
  });

  it('Cancel is a link back to /presets and sends nothing', async () => {
    await openNew();
    expect(screen.getByRole('link', { name: 'Cancel' })).toHaveAttribute('href', '/presets');
    expectNoWrite();
  });
});

describe('Presets: edit', () => {
  async function openEdit(id = 1, others: Preset[] = []): Promise<UserEvent> {
    const user = userEvent.setup();
    seedPresets([fx.preset({ id, name: 'wan-100ms', description: 'Typical WAN', config: fx.config2() }), ...others]);
    renderApp(`/presets/${id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(lbl('Name')).toHaveValue('wan-100ms'));
    watchWrites();
    return user;
  }

  it('loads the stored preset into the form (every config field)', async () => {
    await openEdit(1);
    expect(lbl('Description')).toHaveValue('Typical WAN');
    expect(lbl('Delay distribution')).toHaveValue('GammaDistribution');
    expect(lbl('Mean delay')).toHaveValue(2.5); // 2500 ns
    expect(unitOf(document.body, 'Mean delay')).toBe('\u00b5s');
    expect(lbl('Delay std')).toHaveValue(300);
    expect(unitOf(document.body, 'Delay std')).toBe('ns');
    expect(lbl('Activate correlation')).toBeChecked();
    expect(lbl('Half-life period')).toHaveValue(4); // 4000000 ns
    expect(unitOf(document.body, 'Half-life period')).toBe('ms');
    expect(lbl('Initialization rate')).toHaveValue(8);
    expect(unitOf(document.body, 'Initialization rate')).toBe('Mpps');
    expect(lbl('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(lbl('Rate')).toHaveValue(40);
    expect(unitOf(document.body, 'Rate')).toBe('Gbit/s');
    expect(lbl('Loss model')).toHaveValue('GilbertElliot');
    expect(lbl('Buffer capacity')).toHaveValue(5000);
    expect(lbl('Reordering')).toBeChecked();
    expect(lbl('Seed')).toHaveValue(42);
    expect(lbl('Histogram bins')).toHaveValue(64);
    expect(lbl('Delay statistics')).not.toBeChecked();
  });

  it('Save stores the edited preset under the same id and navigates to the list', async () => {
    const user = await openEdit(1);
    await user.clear(lbl('Name'));
    await user.type(lbl('Name'), 'renamed');
    await typeInto(user, document.body, 'Mean delay', '4242', 'ns');
    await user.click(save());
    expect(writeCount()).toBe(1);
    expect(storedPresets()).toHaveLength(1);
    const sent = storedPresets()[0];
    expect(sent.id).toBe(1);
    expect(sent.name).toBe('renamed');
    expect(sent.description).toBe('Typical WAN');
    expect(must(sent.config.delayer).mean_ns).toBe(4242);
    expect(must(sent.config.delayer).distribution).toBe('GammaDistribution');
    expect(must(sent.config.delayer).initialization_rate_pps).toBe(8000000); // v12: the stored value goes back
    expect(sent.config.buffer_capacity).toBe(5000);
    expect(sent.config.seed).toBe(42);
    expect(await screen.findByRole('heading', { level: 1, name: 'Presets' })).toBeInTheDocument();
  });

  it('renaming to the name of ANOTHER preset is refused and stays on the page; keeping the own name is fine', async () => {
    const user = await openEdit(1, [fx.preset({ id: 2, name: 'taken' })]);
    await fillName(user, 'taken');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/already exists/i);
    expectNoWrite();
    expect(screen.getByRole('heading', { level: 1, name: 'Edit preset' })).toBeInTheDocument();
    await fillName(user, 'wan-100ms');
    await user.click(save());
    expect(writeCount()).toBe(1);
  });

  it('a stored preset without stages opens with the three "Add ..." buttons and saves back without stages', async () => {
    seedPresets([fx.preset({ id: 1, name: 'bare', config: fx.configBare() })]);
    const user = userEvent.setup();
    renderApp('/presets/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(lbl('Name')).toHaveValue('bare'));
    for (const s of ['delayer', 'limiter', 'dropper'])
      expect(screen.getByRole('button', { name: `Add ${s}` })).toBeInTheDocument();
    await user.click(save());
    const c = storedPresets()[0].config;
    for (const k of ['delayer', 'limiter', 'loss_model'] as const) expect(stageOf(c, k)).toBeNull();
  });

  it('an id that is not in the local document shows "not found" and no editor', async () => {
    seedPresets([fx.preset({ id: 1 })]);
    renderApp('/presets/9');
    expect(await screen.findByRole('alert')).toHaveTextContent(/not found/i);
    expect(screen.queryByLabelText('Name')).not.toBeInTheDocument();
  });
});

// v12 (spec12 part B): same required field as in the link editor ("v12" block of src/test/helpers.tsx).
describe('Presets: initialization rate (v12)', () => {
  const RATE = 'Initialization rate';

  it('a new preset shows the number field once, in the config form, and it starts EMPTY (no preset value)', async () => {
    await openNew();
    expect(screen.getAllByLabelText(RATE)).toHaveLength(1);
    expect(lbl(RATE)).toHaveAttribute('type', 'number');
    expect(lbl(RATE)).toHaveValue(null);
    expect(lbl(RATE)).toHaveDisplayValue('');
    expect(unitOf(document.body, RATE)).toBe('Mpps');
  });

  it('saving the untouched form (name given, rate never entered) is refused: alert naming the field, no request', async () => {
    const user = await openNew();
    await fillName(user, 'no-rate');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expectNoWrite();
  });

  it('typing a rate into the empty form makes it saveable (shared helper path)', async () => {
    const user = await openNew();
    await fillValid(user, 'with-rate');
    expect(lbl(RATE)).toHaveValue(Number(TEST_RATE));
    await user.click(save());
    expect(must(lastCreate().config.delayer).initialization_rate_pps).toBe(Number(TEST_RATE));
  });

  it('sends the typed value as a number, also with correlation off', async () => {
    const user = await openNew();
    await fillName(user, 'typed');
    expect(lbl('Activate correlation')).not.toBeChecked();
    await typeRate(user, '3000000');
    await user.click(save());
    const sent = must(lastCreate().config.delayer).initialization_rate_pps;
    expect(sent).toBe(3000000);
    expect(typeof sent).toBe('number');
  });

  it.each(['', '0', '0.5', '-3'])('refuses "%s": alert naming the field, no request', async (v) => {
    const user = await openNew();
    await fillName(user, 'bad');
    if (v === '') await user.clear(lbl(RATE));
    else await typeRate(user, v);
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expectNoWrite();
  });

  it('accepts exactly 1', async () => {
    const user = await openNew();
    await fillName(user, 'one');
    await typeRate(user, '1');
    await user.click(save());
    expect(must(lastCreate().config.delayer).initialization_rate_pps).toBe(1);
  });

  it('edit: a changed value is stored', async () => {
    const user = userEvent.setup();
    seedPresets([fx.preset({ id: 1, name: 'wan-100ms', config: fx.config2() })]);
    renderApp('/presets/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(lbl('Name')).toHaveValue('wan-100ms'));
    await typeRate(user, '5000000');
    await user.click(save());
    expect(must(storedPresets()[0].config.delayer).initialization_rate_pps).toBe(5000000);
  });
});
