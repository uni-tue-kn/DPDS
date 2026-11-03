import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto, PresetInput } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp, TEST_RATE, typeInto, typeRate, unitOf } from '../test/helpers';

// Contract: "Presets page" in the v8 block of src/test/helpers.tsx.
const DISTRIBUTIONS = [
  'DeterministicDistribution', 'LogNormalDistribution', 'NormalDistribution', 'GammaDistribution', 'UniformDistribution',
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
  expect(DISTRIBUTIONS).toContain(c.delayer.distribution);
  for (const v of [c.delayer.mean_ns, c.delayer.std_ns, c.delayer.half_life_period_ns, c.limiter.rate_bps, c.limiter.limit_bit]) {
    expect(Number.isFinite(v)).toBe(true);
  }
  expect(typeof c.delayer.initialization_rate_pps).toBe('number'); // v12: required, finite >= 1
  expect(Number.isFinite(c.delayer.initialization_rate_pps)).toBe(true);
  expect(c.delayer.initialization_rate_pps).toBeGreaterThanOrEqual(1);
  expect(['Spacer', 'TokenBucketFilter']).toContain(c.limiter.mode);
  expect(['Random', 'GilbertElliot']).toContain(c.loss_model.type);
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
    const names = within(nav).getAllByRole('link').map((a) => a.getAttribute('aria-label') ?? a.textContent?.trim());
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
    api.getPresets.mockResolvedValue(twoPresets());
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
    api.getPresets.mockResolvedValue(twoPresets());
    renderApp('/presets');
    expect(await screen.findByRole('link', { name: 'New preset' })).toHaveAttribute('href', '/presets/new');
    expect(screen.getByRole('link', { name: 'Edit wan-100ms' })).toHaveAttribute('href', '/presets/1');
    expect(screen.getByRole('link', { name: 'Edit lossy' })).toHaveAttribute('href', '/presets/2');
  });

  it('shows an alert if the presets cannot be loaded', async () => {
    api.getPresets.mockRejectedValue(new ApiError(500, 'state unreadable'));
    renderApp('/presets');
    expect(await screen.findByRole('alert')).toHaveTextContent('state unreadable');
  });
});

describe('Presets: delete', () => {
  it('asks for confirmation; Cancel does not delete', async () => {
    const user = userEvent.setup();
    api.getPresets.mockResolvedValue(twoPresets());
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('wan-100ms');
    expect(api.deletePreset).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.deletePreset).not.toHaveBeenCalled();
  });

  it('Confirm deletes by id and reloads the list', async () => {
    const user = userEvent.setup();
    api.getPresets.mockResolvedValueOnce(twoPresets()).mockResolvedValue([twoPresets()[1]]);
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    expect(api.deletePreset).toHaveBeenCalledTimes(1);
    expect(api.deletePreset).toHaveBeenCalledWith(1);
    await waitFor(() => expect(screen.queryByText('wan-100ms')).not.toBeInTheDocument());
    expect(screen.getByText('lossy')).toBeInTheDocument();
  });

  it('shows the API error when deleting fails', async () => {
    const user = userEvent.setup();
    api.getPresets.mockResolvedValue(twoPresets());
    api.deletePreset.mockRejectedValue(new ApiError(404, 'preset 1 does not exist'));
    renderApp('/presets');
    await user.click(await screen.findByRole('button', { name: 'Delete wan-100ms' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    expect(await screen.findByText(/preset 1 does not exist/)).toBeInTheDocument();
  });
});

async function openNew(): Promise<UserEvent> {
  const user = userEvent.setup();
  renderApp('/presets/new');
  await screen.findByRole('heading', { level: 1, name: 'New preset' });
  return user;
}

async function fillName(user: UserEvent, name: string) {
  await user.clear(lbl('Name'));
  await user.type(lbl('Name'), name);
}

/** Name plus the initialization rate (a new form starts without one), ready to save. */
async function fillValid(user: UserEvent, name: string) {
  await fillName(user, name);
  await typeRate(user);
}

function lastCreate(): PresetInput {
  expect(api.createPreset).toHaveBeenCalledTimes(1);
  return api.createPreset.mock.calls[0][0];
}

describe('Presets: create', () => {
  it('shows name, description and exactly one direction config form with defaults', async () => {
    await openNew();
    expect(lbl('Name')).toHaveValue('');
    expect(lbl('Description')).toHaveValue('');
    expect(screen.getAllByLabelText('Mean delay')).toHaveLength(1);
    expect(screen.getAllByLabelText('Delay distribution')).toHaveLength(1);
    expect(screen.getByLabelText('Limiter active')).not.toBeChecked();
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
    await user.click(lbl('Limiter active'));
    await user.click(lbl('Reordering'));
    await user.click(save());
    const c = lastCreate().config;
    expect(c.delayer).toMatchObject({ distribution: 'GammaDistribution', mean_ns: 2500, std_ns: 300 });
    expect(c.limiter.activate).toBe(true);
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
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('refuses a whitespace-only name', async () => {
    const user = await openNew();
    await fillName(user, '   ');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('refuses a seed above 2^53-1 like the link editor (alert naming the seed, no request)', async () => {
    const user = await openNew();
    await fillValid(user, 'big-seed');
    await user.type(lbl('Seed'), '9007199254740993');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/seed/i);
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('shows the API 409 message, stays on the page and keeps the values', async () => {
    api.createPreset.mockRejectedValue(new ApiError(409, 'preset name "dup" already exists'));
    const user = await openNew();
    await fillValid(user, 'dup');
    await user.type(lbl('Description'), 'keep me');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('preset name "dup" already exists');
    expect(screen.getByRole('heading', { level: 1, name: 'New preset' })).toBeInTheDocument();
    expect(lbl('Name')).toHaveValue('dup');
    expect(lbl('Description')).toHaveValue('keep me');
  });

  it('shows the API 400 message naming the field', async () => {
    api.createPreset.mockRejectedValue(new ApiError(400, 'config.delayer.std_ns must be >= 0'));
    const user = await openNew();
    await fillValid(user, 'bad');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('config.delayer.std_ns must be >= 0');
  });

  it('Cancel is a link back to /presets and sends nothing', async () => {
    await openNew();
    expect(screen.getByRole('link', { name: 'Cancel' })).toHaveAttribute('href', '/presets');
    expect(api.createPreset).not.toHaveBeenCalled();
  });
});

describe('Presets: edit', () => {
  async function openEdit(id = 1): Promise<UserEvent> {
    const user = userEvent.setup();
    api.getPreset.mockImplementation(async () => fx.preset({ id, name: 'wan-100ms', description: 'Typical WAN', config: fx.config2() }));
    renderApp(`/presets/${id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(lbl('Name')).toHaveValue('wan-100ms'));
    return user;
  }

  it('loads the stored preset into the form (every config field)', async () => {
    await openEdit(1);
    expect(api.getPreset).toHaveBeenCalledWith(1);
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
    expect(lbl('Limiter active')).toBeChecked();
    expect(lbl('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(lbl('Rate')).toHaveValue(40);
    expect(unitOf(document.body, 'Rate')).toBe('Gbit/s');
    expect(lbl('Burst limit')).toHaveValue(120);
    expect(unitOf(document.body, 'Burst limit')).toBe('kbit');
    expect(lbl('Loss model')).toHaveValue('GilbertElliot');
    expect(lbl('Buffer capacity')).toHaveValue(5000);
    expect(lbl('Reordering')).toBeChecked();
    expect(lbl('Seed')).toHaveValue(42);
    expect(lbl('Histogram bins')).toHaveValue(64);
    expect(lbl('Delay statistics')).not.toBeChecked();
  });

  it('Save PUTs the edited preset by id and navigates to the list', async () => {
    const user = await openEdit(1);
    await user.clear(lbl('Name'));
    await user.type(lbl('Name'), 'renamed');
    await typeInto(user, document.body, 'Mean delay', '4242', 'ns');
    await user.click(save());
    expect(api.updatePreset).toHaveBeenCalledTimes(1);
    const [id, sent] = api.updatePreset.mock.calls[0];
    expect(id).toBe(1);
    expect(sent.name).toBe('renamed');
    expect(sent.description).toBe('Typical WAN');
    expect(sent.config.delayer.mean_ns).toBe(4242);
    expect(sent.config.delayer.distribution).toBe('GammaDistribution');
    expect(sent.config.delayer.initialization_rate_pps).toBe(8000000); // v12: the stored value goes back
    expect(sent.config.buffer_capacity).toBe(5000);
    expect(sent.config.seed).toBe(42);
    expect(api.createPreset).not.toHaveBeenCalled();
    expect(await screen.findByRole('heading', { level: 1, name: 'Presets' })).toBeInTheDocument();
  });

  it('shows the API error of a failed update and stays', async () => {
    api.updatePreset.mockRejectedValue(new ApiError(409, 'preset name "x" already exists'));
    const user = await openEdit(1);
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('already exists');
    expect(screen.getByRole('heading', { level: 1, name: 'Edit preset' })).toBeInTheDocument();
  });

  it('shows an alert if the preset cannot be loaded (404)', async () => {
    api.getPreset.mockRejectedValue(new ApiError(404, 'preset 9 does not exist'));
    renderApp('/presets/9');
    expect(await screen.findByRole('alert')).toHaveTextContent('preset 9 does not exist');
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
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('typing a rate into the empty form makes it saveable (shared helper path)', async () => {
    const user = await openNew();
    await fillValid(user, 'with-rate');
    expect(lbl(RATE)).toHaveValue(Number(TEST_RATE));
    await user.click(save());
    expect(lastCreate().config.delayer.initialization_rate_pps).toBe(Number(TEST_RATE));
  });

  it('sends the typed value as a number, also with correlation off', async () => {
    const user = await openNew();
    await fillName(user, 'typed');
    expect(lbl('Activate correlation')).not.toBeChecked();
    await typeRate(user, '3000000');
    await user.click(save());
    const sent = lastCreate().config.delayer.initialization_rate_pps;
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
    expect(api.createPreset).not.toHaveBeenCalled();
  });

  it('accepts exactly 1', async () => {
    const user = await openNew();
    await fillName(user, 'one');
    await typeRate(user, '1');
    await user.click(save());
    expect(lastCreate().config.delayer.initialization_rate_pps).toBe(1);
  });

  it('edit: a changed value is sent by PUT', async () => {
    const user = userEvent.setup();
    api.getPreset.mockImplementation(async () => fx.preset({ id: 1, name: 'wan-100ms', config: fx.config2() }));
    renderApp('/presets/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(lbl('Name')).toHaveValue('wan-100ms'));
    await typeRate(user, '5000000');
    await user.click(save());
    expect(api.updatePreset.mock.calls[0][1].config.delayer.initialization_rate_pps).toBe(5000000);
  });
});
