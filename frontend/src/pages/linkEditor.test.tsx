import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto, Link, LinkInput } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp, section, TEST_RATE, typeInto, typeRate, unitOf } from '../test/helpers';

const DISTRIBUTIONS = [
  'DeterministicDistribution', 'LogNormalDistribution', 'NormalDistribution', 'GammaDistribution', 'UniformDistribution',
];

// v10: one directed link = one config form (contract: "v10" block of src/test/helpers.tsx).
const group = () => screen.getByRole('group', { name: /^Link configuration/ });
const select = (name: string) => screen.getByLabelText(name) as HTMLSelectElement;
const input = (scope: HTMLElement, label: string) => within(scope).getByLabelText(label) as HTMLInputElement;
const save = () => screen.getByRole('button', { name: /^save$/i });
const option = (label: 'Ingress' | 'Egress', pci: RegExp) => within(select(label)).getByRole('option', { name: pci });

/** A stored link of another name that uses 0000:65:00.0 as ingress and 0000:65:00.1 as egress. */
const other = () => fx.link({ id: 10, name: 'other' });

async function openNew(others: Link[] = []): Promise<UserEvent> {
  api.getLinks.mockResolvedValue(others);
  const user = userEvent.setup();
  renderApp('/links/new');
  await screen.findByRole('heading', { level: 1, name: 'New link' });
  await waitFor(() => expect(option('Ingress', /enp101s0f0/)).toBeInTheDocument());
  return user;
}

async function fillName(user: UserEvent, name: string) {
  await user.clear(screen.getByLabelText('Name'));
  await user.type(screen.getByLabelText('Name'), name);
}

/** Name + the two usable ports + the initialization rate (a new form starts without one), ready to save. */
async function fillValid(user: UserEvent, name = 'my-link') {
  await fillName(user, name);
  await user.selectOptions(select('Ingress'), '0000:65:00.0');
  await user.selectOptions(select('Egress'), '0000:65:00.1');
  await typeRate(user);
}

function lastCreate(): LinkInput {
  expect(api.createLink).toHaveBeenCalledTimes(1);
  return api.createLink.mock.calls[0][0];
}

/** Every ConfigDto the frontend sends must be structurally valid for the controller. */
function expectValidConfig(c: ConfigDto) {
  expect(Number.isInteger(c.buffer_capacity)).toBe(true);
  expect(c.buffer_capacity).toBeGreaterThan(0);
  expect(typeof c.reordering).toBe('boolean');
  expect(DISTRIBUTIONS).toContain(c.delayer.distribution);
  expect(Number.isFinite(c.delayer.mean_ns)).toBe(true);
  expect(Number.isFinite(c.delayer.std_ns)).toBe(true);
  expect(typeof c.delayer.activate_correlation).toBe('boolean');
  expect(Number.isFinite(c.delayer.half_life_period_ns)).toBe(true);
  expect(typeof c.delayer.initialization_rate_pps).toBe('number'); // v12: always sent, finite >= 1
  expect(Number.isFinite(c.delayer.initialization_rate_pps)).toBe(true);
  expect(c.delayer.initialization_rate_pps).toBeGreaterThanOrEqual(1);
  expect(['Spacer', 'TokenBucketFilter']).toContain(c.limiter.mode);
  expect(typeof c.limiter.activate).toBe('boolean');
  expect(c.limiter.rate_bps).toBeGreaterThan(0);
  expect(c.limiter.limit_bit).toBeGreaterThanOrEqual(0);
  expect(['Random', 'GilbertElliot']).toContain(c.loss_model.type);
  expect(typeof c.delay_stats).toBe('boolean');
}

/** The old two-port shape must be gone from what the editor sends. */
function expectNewShape(sent: LinkInput) {
  const keys = Object.keys(sent);
  for (const gone of ['ports', 'configs', 'workers']) expect(keys).not.toContain(gone);
  expect(typeof sent.ingress).toBe('string');
  expect(typeof sent.egress).toBe('string');
  expect(sent.config).toBeTypeOf('object');
}

describe('Link editor: interface pickers', () => {
  it('has an Ingress and an Egress picker and none of the old two-port controls', async () => {
    await openNew();
    expect(select('Ingress')).toBeInTheDocument();
    expect(select('Egress')).toBeInTheDocument();
    for (const gone of ['Number of ports', 'Port A', 'Port B', 'Port A (typed)', 'Port B (typed)', 'Worker lcores']) {
      expect(screen.queryByLabelText(gone)).not.toBeInTheDocument();
    }
  });

  it('lists the interfaces of /api/interfaces in both pickers, labelled by netdev (v16)', async () => {
    await openNew();
    for (const label of ['Ingress', 'Egress']) {
      const opts = within(select(label)).getAllByRole('option').map((o) => o.textContent ?? '');
      for (const netdev of ['enp101s0f0', 'enp101s0f1', 'enp179s0f0', 'eno1']) {
        expect(opts.some((t) => t.startsWith(netdev))).toBe(true);
      }
    }
  });

  it('shows the netdev only, no PCI address in the option text, while the value stays the PCI address (v16)', async () => {
    await openNew();
    for (const label of ['Ingress', 'Egress'] as const) {
      const o = option(label, /enp101s0f0/) as HTMLOptionElement;
      expect(o.textContent).toBe('enp101s0f0');
      expect(o.value).toBe('0000:65:00.0');
      expect(option(label, /enp101s0f1/).textContent).toBe('enp101s0f1');
      for (const t of within(select(label)).getAllByRole('option').map((x) => x.textContent ?? '')) {
        expect(t).not.toMatch(/0000:/);
        expect(t).not.toContain('(');
      }
    }
  });

  it('an interface without a netdev (vfio-bound) is labelled by its PCI address', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({ pci: '0000:65:00.0', netdev: null }), fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' }),
    ]);
    api.getLinks.mockResolvedValue([]);
    renderApp('/links/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    expect(await within(select('Ingress')).findByRole('option', { name: '0000:65:00.0' })).toBeInTheDocument();
    expect(within(select('Egress')).getByRole('option', { name: 'enp101s0f1' })).toBeInTheDocument();
  });

  it('an unusable interface keeps its netdev label plus the reason, still without the PCI address', async () => {
    await openNew();
    const o = option('Ingress', /enp179s0f0/);
    expect(o.textContent).toMatch(/^enp179s0f0 - .*bind to vfio-pci/);
    expect(o.textContent).not.toMatch(/0000:b3/);
  });

  it('starts with nothing selected', async () => {
    await openNew();
    expect(select('Ingress').value).toBe('');
    expect(select('Egress').value).toBe('');
  });

  it.each(['Ingress', 'Egress'] as const)(
    'disables unusable and management interfaces in the %s picker and shows the reason',
    async (label) => {
      await openNew();
      expect(option(label, /enp101s0f0/)).toBeEnabled();
      const unusable = option(label, /enp179s0f0/);
      expect(unusable).toBeDisabled();
      expect(unusable).toHaveTextContent('bind to vfio-pci');
      const mgmt = option(label, /eno1/);
      expect(mgmt).toBeDisabled();
      expect(mgmt).toHaveTextContent(/management NIC/);
    },
  );

  it('shows an alert if the interfaces cannot be loaded', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/links/new');
    expect(await screen.findByRole('alert')).toHaveTextContent('sysfs unreadable');
  });
});

describe('Link editor: ports already used by other links', () => {
  it('Ingress picker: a port another link uses as ingress is disabled with the reason and the link name', async () => {
    await openNew([other()]);
    const used = option('Ingress', /enp101s0f0/);
    expect(used).toBeDisabled();
    expect(used).toHaveTextContent(/already used as ingress by/i);
    expect(used).toHaveTextContent('other');
  });

  it('Ingress picker: a port another link only uses as egress stays selectable (ring A->B, B->C)', async () => {
    await openNew([other()]); // other: .0 -> .1
    expect(option('Ingress', /enp101s0f1/)).toBeEnabled();
  });

  it('Egress picker: a port another link uses as egress is disabled with the reason and the link name', async () => {
    await openNew([other()]);
    const used = option('Egress', /enp101s0f1/);
    expect(used).toBeDisabled();
    expect(used).toHaveTextContent(/already used as egress by/i);
    expect(used).toHaveTextContent('other');
  });

  it('Egress picker: a port another link only uses as ingress stays selectable', async () => {
    await openNew([other()]);
    expect(option('Egress', /enp101s0f0/)).toBeEnabled();
  });

  it('the Ingress reason says "ingress", the Egress reason says "egress" (never the other word)', async () => {
    await openNew([other()]);
    expect(option('Ingress', /enp101s0f0/).textContent).not.toMatch(/already used as egress/i);
    expect(option('Egress', /enp101s0f1/).textContent).not.toMatch(/already used as ingress/i);
  });

  it('a port is only disabled for its own role: the reverse link of an existing link can be created', async () => {
    const user = await openNew([other()]); // other: .0 -> .1
    await fillName(user, 'back');
    await typeRate(user);
    await user.selectOptions(select('Ingress'), '0000:65:00.1');
    await user.selectOptions(select('Egress'), '0000:65:00.0');
    await user.click(save());
    const sent = lastCreate();
    expect(sent.ingress).toBe('0000:65:00.1');
    expect(sent.egress).toBe('0000:65:00.0');
  });

  it('names the link for each use when two different links hold the two roles of one port', async () => {
    // .0 is ingress of "first" (.0 -> .1) and egress of "second" (.1 -> .0)
    await openNew([fx.link({ id: 10, name: 'first' }), fx.linkRev({ id: 11, name: 'second' })]);
    expect(option('Ingress', /enp101s0f0/)).toBeDisabled();
    expect(option('Ingress', /enp101s0f0/)).toHaveTextContent('first');
    expect(option('Egress', /enp101s0f0/)).toBeDisabled();
    expect(option('Egress', /enp101s0f0/)).toHaveTextContent('second');
    expect(option('Ingress', /enp101s0f1/)).toBeDisabled();
    expect(option('Ingress', /enp101s0f1/)).toHaveTextContent('second');
    expect(option('Egress', /enp101s0f1/)).toBeDisabled();
    expect(option('Egress', /enp101s0f1/)).toHaveTextContent('first');
  });

  it('with no other links nothing is disabled except unusable interfaces', async () => {
    await openNew([]);
    for (const label of ['Ingress', 'Egress'] as const) {
      expect(option(label, /enp101s0f0/)).toBeEnabled();
      expect(option(label, /enp101s0f1/)).toBeEnabled();
      expect(option(label, /enp101s0f0/).textContent).not.toMatch(/already used/i);
    }
  });

  it('a used-but-unusable port keeps its own (driver) reason', async () => {
    await openNew([fx.link({ id: 10, name: 'other', ingress: '0000:b3:00.0' })]);
    const o = option('Ingress', /enp179s0f0/);
    expect(o).toBeDisabled();
    expect(o).toHaveTextContent('bind to vfio-pci');
  });

  it('editing a link: its own ports stay selectable, other links\' ports are disabled', async () => {
    const mine = fx.link(); // id 1: .0 -> .1
    const rev = fx.linkRev(); // id 3: .1 -> .0
    api.getLinks.mockResolvedValue([mine, rev]);
    api.getLink.mockResolvedValue(mine);
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.0'));
    expect(select('Egress').value).toBe('0000:65:00.1');
    expect(option('Ingress', /enp101s0f0/)).toBeEnabled(); // mine
    expect(option('Egress', /enp101s0f1/)).toBeEnabled(); // mine
    expect(option('Ingress', /enp101s0f1/)).toBeDisabled(); // b-a's ingress
    expect(option('Ingress', /enp101s0f1/)).toHaveTextContent('b-a');
    expect(option('Egress', /enp101s0f0/)).toBeDisabled(); // b-a's egress
    expect(option('Egress', /enp101s0f0/)).toHaveTextContent('b-a');
  });

  it('a hairpin link of another link disables that port in both pickers', async () => {
    await openNew([fx.link({ id: 10, name: 'h', ingress: '0000:65:00.0', egress: '0000:65:00.0' })]);
    expect(option('Ingress', /enp101s0f0/)).toBeDisabled();
    expect(option('Egress', /enp101s0f0/)).toBeDisabled();
    expect(option('Ingress', /enp101s0f1/)).toBeEnabled();
    expect(option('Egress', /enp101s0f1/)).toBeEnabled();
  });

  it('the editor still works when the links cannot be loaded (nothing disabled)', async () => {
    api.getLinks.mockRejectedValue(new ApiError(500, 'links unreadable'));
    const user = userEvent.setup();
    renderApp('/links/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(option('Ingress', /enp101s0f0/)).toBeInTheDocument());
    expect(option('Ingress', /enp101s0f0/)).toBeEnabled();
    await fillValid(user);
    await user.click(save());
    expect(lastCreate().ingress).toBe('0000:65:00.0');
  });

  it('shows a 409 conflict message from the controller (port already used)', async () => {
    api.createLink.mockRejectedValue(new ApiError(409, 'port 0000:65:00.0 is already used as ingress by link "other"'));
    const user = await openNew([]); // e.g. a link created meanwhile
    await fillValid(user, 'dup');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/already used as ingress by link "other"/);
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
  });
});

describe('Link editor: create', () => {
  it('starts with a single link configuration form', async () => {
    await openNew();
    expect(screen.getAllByRole('group', { name: /^Link configuration/ })).toHaveLength(1);
    expect(screen.getAllByLabelText('Mean delay')).toHaveLength(1);
    expect(screen.queryByRole('group', { name: /^Direction \d/ })).not.toBeInTheDocument();
  });

  it('creates a link with valid default config and navigates to the list', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.click(save());
    const sent = lastCreate();
    expectNewShape(sent);
    expect(sent.name).toBe('my-link');
    expect(sent.ingress).toBe('0000:65:00.0');
    expect(sent.egress).toBe('0000:65:00.1');
    expectValidConfig(sent.config);
    expect(sent.config.delay_stats).toBe(true);
    expect(sent.config.reordering).toBe(false);
    expect(sent.config.loss_model.type).toBe('Random');
    expect(sent.worker ?? null).toBeNull();
    expect(sent.config.seed ?? null).toBeNull();
    expect(sent.config.histogram?.bins ?? null).toBeNull();
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('a hairpin link (same port as ingress and egress) can be saved', async () => {
    const user = await openNew();
    await fillName(user, 'loop');
    await typeRate(user);
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.selectOptions(select('Egress'), '0000:65:00.0');
    await user.click(save());
    const sent = lastCreate();
    expect(sent.ingress).toBe('0000:65:00.0');
    expect(sent.egress).toBe('0000:65:00.0');
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('sends every config field as typed (each time/rate/size field in its own unit)', async () => {
    const user = await openNew();
    await fillValid(user, 'full');
    const g = group();
    await user.selectOptions(within(g).getByLabelText('Delay distribution'), 'GammaDistribution');
    await typeInto(user, g, 'Mean delay', '2.5', 'ms');
    await typeInto(user, g, 'Delay std', '120', '\u00b5s');
    await user.click(input(g, 'Activate correlation'));
    await typeInto(user, g, 'Half-life period', '8', 'ms');
    await typeInto(user, g, 'Initialization rate', '4', 'Mpps');
    await user.click(input(g, 'Limiter active'));
    await user.selectOptions(within(g).getByLabelText('Limiter mode'), 'TokenBucketFilter');
    await typeInto(user, g, 'Rate', '100', 'Gbit/s');
    await typeInto(user, g, 'Burst limit', '240', 'kbit');
    await user.clear(input(g, 'Buffer capacity'));
    await user.type(input(g, 'Buffer capacity'), '250000');
    await user.click(input(g, 'Reordering'));
    await user.clear(input(g, 'Seed'));
    await user.type(input(g, 'Seed'), '1234');
    await user.type(input(g, 'Histogram bins'), '128');
    await typeInto(user, g, 'Histogram lo', '1', 'ms');
    await typeInto(user, g, 'Histogram hi', '4000', '\u00b5s');
    await user.click(input(g, 'Delay statistics')); // default on -> off
    await user.clear(input(g, 'Random loss probability'));
    await user.type(input(g, 'Random loss probability'), '0.05');

    await user.click(save());
    const c = lastCreate().config;
    expect(c.delayer).toEqual({
      distribution: 'GammaDistribution', mean_ns: 2500000, std_ns: 120000,
      activate_correlation: true, half_life_period_ns: 8000000, initialization_rate_pps: 4000000,
    });
    expect(c.limiter).toEqual({ mode: 'TokenBucketFilter', activate: true, rate_bps: 100000000000, limit_bit: 240000 });
    expect(c.buffer_capacity).toBe(250000);
    expect(c.reordering).toBe(true);
    expect(c.seed).toBe(1234);
    expect(c.histogram).toEqual({ bins: 128, lo_ns: 1000000, hi_ns: 4000000 });
    expect(c.delay_stats).toBe(false);
    expect(c.loss_model).toEqual({ type: 'Random', p: 0.05 });
  });

  it('sends a Gilbert-Elliot loss model as typed', async () => {
    const user = await openNew();
    await fillValid(user);
    const g = group();
    await user.selectOptions(within(g).getByLabelText('Loss model'), 'GilbertElliot');
    await user.clear(input(g, 'Gilbert-Elliot p'));
    await user.type(input(g, 'Gilbert-Elliot p'), '0.01');
    await user.clear(input(g, 'Gilbert-Elliot r'));
    await user.type(input(g, 'Gilbert-Elliot r'), '0.2');
    await user.clear(input(g, 'Gilbert-Elliot k'));
    await user.type(input(g, 'Gilbert-Elliot k'), '0.9');
    await user.clear(input(g, 'Gilbert-Elliot h'));
    await user.type(input(g, 'Gilbert-Elliot h'), '0.3');
    await user.click(input(g, 'Gilbert-Elliot starts in good state')); // default on -> off
    await user.click(save());
    const c = lastCreate().config;
    expect(c.loss_model).toEqual({ type: 'GilbertElliot', p: 0.01, r: 0.2, k: 0.9, h: 0.3, state_is_good: false });
    expect(c.limiter.activate).toBe(false);
    expect(c.delay_stats).toBe(true);
  });

  it('shows only the fields of the selected loss model', async () => {
    const user = await openNew();
    const g = section('Dropper');
    expect(within(g).getByLabelText('Random loss probability')).toBeInTheDocument();
    expect(within(g).queryByLabelText('Gilbert-Elliot p')).not.toBeInTheDocument();
    await user.selectOptions(within(g).getByLabelText('Loss model'), 'GilbertElliot');
    expect(within(g).queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    for (const n of ['p', 'r', 'k', 'h']) expect(within(g).getByLabelText(`Gilbert-Elliot ${n}`)).toBeInTheDocument();
    expect(within(g).getByLabelText('Gilbert-Elliot starts in good state')).toBeChecked();
  });

  it('offers exactly the five delay distributions and the two limiter modes', async () => {
    await openNew();
    const g = group();
    const dist = within(within(g).getByLabelText('Delay distribution') as HTMLElement).getAllByRole('option');
    expect(dist.map((o) => (o as HTMLOptionElement).value).sort()).toEqual([...DISTRIBUTIONS].sort());
    const modes = within(within(g).getByLabelText('Limiter mode') as HTMLElement).getAllByRole('option');
    expect(modes.map((o) => (o as HTMLOptionElement).value).sort()).toEqual(['Spacer', 'TokenBucketFilter']);
    const loss = within(within(g).getByLabelText('Loss model') as HTMLElement).getAllByRole('option');
    expect(loss.map((o) => (o as HTMLOptionElement).value).sort()).toEqual(['GilbertElliot', 'Random']);
  });

  it('only some histogram fields filled: the others are null/absent', async () => {
    const user = await openNew();
    await fillValid(user, 'h');
    await user.type(input(group(), 'Histogram bins'), '64');
    await user.click(save());
    const h = lastCreate().config.histogram;
    expect(h?.bins).toBe(64);
    expect(h?.lo_ns ?? null).toBeNull();
    expect(h?.hi_ns ?? null).toBeNull();
  });

  it('worker override: one optional lcore number', async () => {
    const user = await openNew();
    expect(screen.getByLabelText('Worker lcore')).toHaveValue(null);
    await fillValid(user, 'w');
    await user.type(screen.getByLabelText('Worker lcore'), '7');
    await user.click(save());
    expect(lastCreate().worker).toBe(7);
  });

  it('an empty worker override means automatic (null/absent)', async () => {
    const user = await openNew();
    await fillValid(user, 'w');
    await user.click(save());
    expect(lastCreate().worker ?? null).toBeNull();
  });

  // The worker field is <input type="number">: the browser/jsdom drop non-numeric characters while typing
  // ("7, 2" arrives as "72", "abc" as ""), so only values a number input can deliver are refused here.
  it.each(['-1', '-7', '1.5', '0.5'])('refuses the worker value "%s": alert mentioning the worker, no request', async (v) => {
    const user = await openNew();
    await fillValid(user, 'w');
    await user.type(screen.getByLabelText('Worker lcore'), v);
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/worker/i);
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
  });

  it('non-numeric text cannot be entered into the worker field: it stays empty and means automatic', async () => {
    const user = await openNew();
    await fillValid(user, 'w');
    await user.type(screen.getByLabelText('Worker lcore'), 'abc');
    expect(screen.getByLabelText('Worker lcore')).toHaveValue(null);
    await user.click(save());
    expect(lastCreate().worker ?? null).toBeNull();
  });

  it('does not call the API without a name', async () => {
    const user = await openNew();
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.selectOptions(select('Egress'), '0000:65:00.1');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
  });

  it('does not call the API without an ingress or without an egress', async () => {
    const user = await openNew();
    await fillName(user, 'noports');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    await user.selectOptions(select('Ingress'), '');
    await user.selectOptions(select('Egress'), '0000:65:00.1');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('shows the API validation error, stays on the page and keeps the form values', async () => {
    api.createLink.mockRejectedValue(new ApiError(400, 'config.delayer.std_ns must be finite and >= 0'));
    const user = await openNew();
    await fillValid(user, 'bad');
    await user.clear(input(group(), 'Buffer capacity'));
    await user.type(input(group(), 'Buffer capacity'), '777');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('config.delayer.std_ns must be finite and >= 0');
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue('bad');
    expect(select('Ingress').value).toBe('0000:65:00.0');
    expect(select('Egress').value).toBe('0000:65:00.1');
    expect(input(group(), 'Buffer capacity')).toHaveValue(777);
    api.createLink.mockImplementation(async (i) => ({ id: 3, ...i }));
    await user.click(save());
    expect(api.createLink).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('Cancel returns to the list without saving', async () => {
    const user = await openNew();
    await user.click(screen.getByRole('link', { name: /^cancel$/i }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('does not submit twice on a double click of Save', async () => {
    api.createLink.mockImplementation(() => new Promise(() => undefined));
    const user = await openNew();
    await fillValid(user, 'once');
    const btn = save();
    await user.click(btn);
    await user.click(btn);
    expect(api.createLink).toHaveBeenCalledTimes(1);
  });
});

describe('Link editor: edit', () => {
  async function openEdit(link = fx.link(), others: Link[] = []) {
    api.getLink.mockResolvedValue(link);
    api.getLinks.mockResolvedValue([link, ...others]);
    const user = userEvent.setup();
    renderApp(`/links/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    return user;
  }

  it('populates name, ingress, egress and the worker override', async () => {
    await openEdit(fx.link({ worker: 7 }));
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.0'));
    expect(select('Egress').value).toBe('0000:65:00.1');
    expect(screen.getByLabelText('Worker lcore')).toHaveValue(7);
  });

  it('an unset worker shows an empty field', async () => {
    await openEdit();
    expect(screen.getByLabelText('Worker lcore')).toHaveValue(null);
  });

  it('populates every field of the config (default-ish fixture) with display units', async () => {
    await openEdit();
    const g = group();
    expect(within(g).getByLabelText('Delay distribution')).toHaveValue('NormalDistribution');
    expect(input(g, 'Mean delay')).toHaveValue(1); // 1000 ns
    expect(unitOf(g, 'Mean delay')).toBe('\u00b5s');
    expect(input(g, 'Delay std')).toHaveValue(10); // 10 ns
    expect(unitOf(g, 'Delay std')).toBe('ns');
    expect(input(g, 'Activate correlation')).not.toBeChecked();
    expect(input(g, 'Half-life period')).toHaveValue(0);
    expect(unitOf(g, 'Half-life period')).toBe('ms'); // 0 -> the default unit
    expect(input(g, 'Initialization rate')).toHaveValue(12.5); // 12500000 pps
    expect(unitOf(g, 'Initialization rate')).toBe('Mpps');
    expect(input(g, 'Limiter active')).not.toBeChecked();
    expect(within(g).getByLabelText('Limiter mode')).toHaveValue('Spacer');
    expect(input(g, 'Rate')).toHaveValue(1); // 1e9 bit/s
    expect(unitOf(g, 'Rate')).toBe('Gbit/s');
    expect(input(g, 'Burst limit')).toHaveValue(0);
    expect(unitOf(g, 'Burst limit')).toBe('Mbit'); // 0 -> the default unit
    expect(within(g).getByLabelText('Loss model')).toHaveValue('Random');
    expect(input(g, 'Random loss probability')).toHaveValue(0);
    expect(input(g, 'Buffer capacity')).toHaveValue(1000);
    expect(input(g, 'Reordering')).not.toBeChecked();
    expect(input(g, 'Seed')).toHaveDisplayValue('');
    expect(input(g, 'Histogram bins')).toHaveValue(null);
    expect(input(g, 'Histogram lo')).toHaveValue(null);
    expect(unitOf(g, 'Histogram lo')).toBe('ms'); // empty -> the default unit
    expect(input(g, 'Histogram hi')).toHaveValue(null);
    expect(unitOf(g, 'Histogram hi')).toBe('ms');
    expect(input(g, 'Delay statistics')).toBeChecked();
  });

  it('populates every field of the config (Gilbert-Elliot, TBF, histogram, delay_stats off) with display units', async () => {
    await openEdit(fx.linkRev());
    const g = group();
    expect(select('Ingress').value).toBe('0000:65:00.1');
    expect(select('Egress').value).toBe('0000:65:00.0');
    expect(within(g).getByLabelText('Delay distribution')).toHaveValue('GammaDistribution');
    expect(input(g, 'Mean delay')).toHaveValue(2.5); // 2500 ns
    expect(unitOf(g, 'Mean delay')).toBe('\u00b5s');
    expect(input(g, 'Delay std')).toHaveValue(300);
    expect(unitOf(g, 'Delay std')).toBe('ns');
    expect(input(g, 'Activate correlation')).toBeChecked();
    expect(input(g, 'Half-life period')).toHaveValue(4); // 4000000 ns
    expect(unitOf(g, 'Half-life period')).toBe('ms');
    expect(input(g, 'Initialization rate')).toHaveValue(8);
    expect(unitOf(g, 'Initialization rate')).toBe('Mpps');
    expect(input(g, 'Limiter active')).toBeChecked();
    expect(within(g).getByLabelText('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(input(g, 'Rate')).toHaveValue(40); // 4e10 bit/s
    expect(unitOf(g, 'Rate')).toBe('Gbit/s');
    expect(input(g, 'Burst limit')).toHaveValue(120); // 120000 bit
    expect(unitOf(g, 'Burst limit')).toBe('kbit');
    expect(within(g).getByLabelText('Loss model')).toHaveValue('GilbertElliot');
    expect(input(g, 'Gilbert-Elliot p')).toHaveValue(0.01);
    expect(input(g, 'Gilbert-Elliot r')).toHaveValue(0.2);
    expect(input(g, 'Gilbert-Elliot k')).toHaveValue(0.9);
    expect(input(g, 'Gilbert-Elliot h')).toHaveValue(0.3);
    expect(input(g, 'Gilbert-Elliot starts in good state')).not.toBeChecked();
    expect(input(g, 'Buffer capacity')).toHaveValue(5000);
    expect(input(g, 'Reordering')).toBeChecked();
    expect(input(g, 'Seed')).toHaveDisplayValue('42');
    expect(input(g, 'Histogram bins')).toHaveValue(64);
    expect(input(g, 'Histogram lo')).toHaveValue(100); // 100 ns
    expect(unitOf(g, 'Histogram lo')).toBe('ns');
    expect(input(g, 'Histogram hi')).toHaveValue(9); // 9000 ns
    expect(unitOf(g, 'Histogram hi')).toBe('\u00b5s');
    expect(input(g, 'Delay statistics')).not.toBeChecked();
  });

  it('saving without changes sends the same values back via updateLink(id, ...)', async () => {
    const link = fx.linkRev({ worker: 4 });
    const user = await openEdit(link);
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.1'));
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(api.updateLink).toHaveBeenCalledTimes(1);
    const [id, sent] = api.updateLink.mock.calls[0];
    expect(id).toBe(3);
    expectNewShape(sent);
    expect(sent.name).toBe('b-a');
    expect(sent.ingress).toBe(link.ingress);
    expect(sent.egress).toBe(link.egress);
    expect(sent.worker).toBe(4);
    expect(sent.config.delayer).toEqual(link.config.delayer);
    expect(sent.config.limiter).toEqual(link.config.limiter);
    expect(sent.config.loss_model).toEqual(link.config.loss_model);
    expect(sent.config.buffer_capacity).toBe(5000);
    expect(sent.config.seed).toBe(42);
    expect(sent.config.histogram).toEqual({ bins: 64, lo_ns: 100, hi_ns: 9000 });
    expect(sent.config.delay_stats).toBe(false);
    expect(sent.config.reordering).toBe(true);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('clearing the worker sends null/absent', async () => {
    const user = await openEdit(fx.link({ worker: 7 }));
    await user.clear(screen.getByLabelText('Worker lcore'));
    await user.click(save());
    expect(api.updateLink.mock.calls[0][1].worker ?? null).toBeNull();
  });

  it('changing ingress and egress is sent', async () => {
    const user = await openEdit(fx.link());
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.0'));
    await user.selectOptions(select('Ingress'), '0000:65:00.1');
    await user.selectOptions(select('Egress'), '0000:65:00.0');
    await user.click(save());
    const sent = api.updateLink.mock.calls[0][1];
    expect(sent.ingress).toBe('0000:65:00.1');
    expect(sent.egress).toBe('0000:65:00.0');
  });

  it('clearing the seed sends null; clearing the histogram fields sends no histogram values', async () => {
    const user = await openEdit(fx.linkRev());
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.1'));
    const g = group();
    await user.clear(input(g, 'Seed'));
    await user.clear(input(g, 'Histogram bins'));
    await user.clear(input(g, 'Histogram lo'));
    await user.clear(input(g, 'Histogram hi'));
    await user.click(save());
    const c = api.updateLink.mock.calls[0][1].config;
    expect(c.seed ?? null).toBeNull();
    expect(c.histogram?.bins ?? null).toBeNull();
    expect(c.histogram?.lo_ns ?? null).toBeNull();
    expect(c.histogram?.hi_ns ?? null).toBeNull();
  });

  it('a stored hairpin on a non-interface port (vdev) stays selected in both pickers and is sent back unchanged', async () => {
    const user = await openEdit(fx.loopLink({ id: 5 }));
    expect(select('Ingress').value).toBe('net_null0');
    expect(select('Egress').value).toBe('net_null0');
    await user.click(save());
    const [id, sent] = api.updateLink.mock.calls[0];
    expect(id).toBe(5);
    expect(sent.ingress).toBe('net_null0');
    expect(sent.egress).toBe('net_null0');
  });

  it('shows API errors of update (404 link vanished)', async () => {
    api.updateLink.mockRejectedValue(new ApiError(404, 'link 1 does not exist'));
    const user = await openEdit();
    await waitFor(() => expect(select('Ingress').value).toBe('0000:65:00.0'));
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('link 1 does not exist');
  });

  it('shows an alert if the link cannot be loaded', async () => {
    api.getLink.mockRejectedValue(new ApiError(404, 'link 9 does not exist'));
    renderApp('/links/9');
    expect(await screen.findByRole('alert')).toHaveTextContent('link 9 does not exist');
  });
});

// v16: the typed "Ingress (typed)" / "Egress (typed)" inputs are gone. What they covered (vdev and undiscovered ports,
// the port rule, hairpins of vdevs) is now reached through a STORED link whose port is not a discovered interface.
describe('Link editor: ports come only from the pickers (v16)', () => {
  async function openStored(link: Link) {
    api.getLink.mockResolvedValue(link);
    api.getLinks.mockResolvedValue([link]);
    const user = userEvent.setup();
    renderApp(`/links/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    return user;
  }
  const selectedText = (label: 'Ingress' | 'Egress') =>
    (within(select(label)).getByRole('option', { selected: true }) as HTMLOptionElement).textContent;

  it('has no typed port inputs, no typed labels and no override hint (new and edit form)', async () => {
    await openNew();
    for (const gone of ['Ingress (typed)', 'Egress (typed)']) expect(screen.queryByLabelText(gone)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/\(typed\)/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/overrides the picker/i)).not.toBeInTheDocument();
    expect(screen.queryByPlaceholderText(/net_null0/)).not.toBeInTheDocument();
    expect(select('Ingress')).toBeInTheDocument();
    expect(select('Egress')).toBeInTheDocument();
  });

  it('the edit form has no typed port inputs either', async () => {
    await openStored(fx.loopLink({ id: 5 }));
    expect(screen.queryByLabelText(/\(typed\)/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/overrides the picker/i)).not.toBeInTheDocument();
  });

  it.each(['0000:af:00.0', '0000:65:00.7', 'net_null0', 'net_null12', 'net_ring1', 'net_ring007'])(
    'a stored ingress %s that is not a discovered interface is shown selected and sent back unchanged',
    async (port) => {
      const user = await openStored(fx.link({ id: 6, ingress: port }));
      expect(select('Ingress').value).toBe(port);
      expect(selectedText('Ingress')).toBe(port);
      expect(within(select('Ingress')).getAllByRole('option', { name: port })).toHaveLength(1);
      expect(option('Ingress', new RegExp(port.replace(/\./g, '\\.')))).toBeEnabled();
      await user.click(save());
      const sent = api.updateLink.mock.calls[0][1];
      expect(sent.ingress).toBe(port);
      expect(sent.egress).toBe('0000:65:00.1');
    },
  );

  it.each(['0000:af:00.0', 'net_null12', 'net_ring1'])('a stored egress %s that is not a discovered interface is shown selected and sent back unchanged', async (port) => {
    const user = await openStored(fx.link({ id: 6, egress: port }));
    expect(select('Egress').value).toBe(port);
    expect(selectedText('Egress')).toBe(port);
    await user.click(save());
    const sent = api.updateLink.mock.calls[0][1];
    expect(sent.egress).toBe(port);
    expect(sent.ingress).toBe('0000:65:00.0');
  });

  it('a stored vdev pair (net_null0 -> net_ring1) keeps both ports', async () => {
    const user = await openStored(fx.link({ id: 6, ingress: 'net_null0', egress: 'net_ring1' }));
    expect(select('Ingress').value).toBe('net_null0');
    expect(select('Egress').value).toBe('net_ring1');
    await user.click(save());
    const sent = api.updateLink.mock.calls[0][1];
    expect([sent.ingress, sent.egress]).toEqual(['net_null0', 'net_ring1']);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('a port that is a discovered interface is not duplicated as an extra option (4 interfaces + placeholder)', async () => {
    await openStored(fx.link());
    expect(within(select('Ingress')).getAllByRole('option')).toHaveLength(5);
    expect(within(select('Egress')).getAllByRole('option')).toHaveLength(5);
  });

  it('the stored unknown port is an option of its own picker only, not of the other one', async () => {
    await openStored(fx.link({ id: 6, ingress: 'net_null0', egress: '0000:65:00.1' }));
    expect(within(select('Ingress')).getAllByRole('option')).toHaveLength(6);
    expect(within(select('Egress')).queryByRole('option', { name: 'net_null0' })).not.toBeInTheDocument();
  });

  it('choosing a discovered interface replaces the stored unknown port', async () => {
    const user = await openStored(fx.loopLink({ id: 5 }));
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.click(save());
    const sent = api.updateLink.mock.calls[0][1];
    expect(sent.ingress).toBe('0000:65:00.0');
    expect(sent.egress).toBe('net_null0');
  });

  // The port rule still applies to what is sent: a stored value the controller would reject is refused here.
  it.each(['net_pcap0,tx_pcap=/x', 'eth0', '0000:65:00.8'])('a stored ingress %s is refused on save: alert naming the ingress, no request', async (port) => {
    const user = await openStored(fx.link({ id: 6, ingress: port }));
    expect(select('Ingress').value).toBe(port);
    await user.click(save());
    expect(api.updateLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/ingress/i);
  });

  it.each(['net_ring1,nodeaction=x:0:CREATE', 'enp101s0f0', '0000:65:00'])('a stored egress %s is refused on save: alert naming the egress, no request', async (port) => {
    const user = await openStored(fx.link({ id: 6, egress: port }));
    await user.click(save());
    expect(api.updateLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/egress/i);
  });

  it('a refused stored port can be fixed through the picker and then saved', async () => {
    const user = await openStored(fx.link({ id: 6, egress: 'eth0' }));
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/egress/i);
    await user.selectOptions(select('Egress'), '0000:65:00.1');
    await user.click(save());
    expect(api.updateLink.mock.calls[0][1].egress).toBe('0000:65:00.1');
  });
});

describe('Link editor: seeds beyond 2^53-1 (F5)', () => {
  async function withSeed(seed: string) {
    const user = await openNew();
    await fillValid(user, 's');
    await user.type(input(group(), 'Seed'), seed);
    await user.click(save());
    return user;
  }

  it('sends the largest safe seed 9007199254740991 exactly', async () => {
    await withSeed('9007199254740991');
    expect(lastCreate().config.seed).toBe(9007199254740991);
  });

  it.each(['9007199254740992', '9007199254740993', '18446744073709551615', '99999999999999999999'])(
    'seed %s is refused with a validation message and not sent',
    async (seed) => {
      await withSeed(seed);
      expect(api.createLink).not.toHaveBeenCalled();
      expect(await screen.findByRole('alert')).toHaveTextContent(/seed/i);
      expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    },
  );

  it('seed 0 and a small seed are still fine', async () => {
    await withSeed('0');
    expect(lastCreate().config.seed).toBe(0);
  });

  it('a refused seed can be corrected and saved', async () => {
    const user = await withSeed('9007199254740992');
    await screen.findByRole('alert');
    await user.clear(input(group(), 'Seed'));
    await user.type(input(group(), 'Seed'), '42');
    await user.click(save());
    expect(lastCreate().config.seed).toBe(42);
  });
});

describe('Link editor: labels (v10)', () => {
  it('the config fieldset is named "Link configuration" and has no "Direction d" legend', async () => {
    await openNew();
    expect(screen.getAllByRole('group', { name: /^Link configuration/ })).toHaveLength(1);
    expect(screen.queryByRole('group', { name: /^Direction/ })).not.toBeInTheDocument();
  });

  it('validation messages name the offending field, without a "Direction d" prefix', async () => {
    const user = await openNew();
    await fillValid(user, 'v');
    await user.clear(input(group(), 'Buffer capacity'));
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/Buffer capacity/);
    expect(alert).not.toHaveTextContent(/Direction \d/);
  });

  it('a second validation message names its own field', async () => {
    const user = await openNew();
    await fillValid(user, 'v');
    await user.clear(input(group(), 'Mean delay'));
    await user.click(save());
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/Mean delay/);
    expect(alert).not.toHaveTextContent(/\(ns\)/); // v16: messages name the field, not a unit
  });

  it('all element ids on the page are unique (no label-derived control ids colliding)', async () => {
    await openNew();
    const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const l of ['Ingress', 'Egress', 'Worker lcore', 'Mean delay unit', 'Delay std unit', 'Rate unit']) {
      expect(screen.getByLabelText(l)).toBeInTheDocument();
    }
  });
});

// v12 (spec12 part B): the correlation initialization rate is a required config field, never a hidden constant.
describe('Link editor: initialization rate (v12)', () => {
  const RATE = 'Initialization rate';
  const rate = () => input(group(), RATE);

  async function openEdit(link = fx.link()) {
    api.getLink.mockResolvedValue(link);
    api.getLinks.mockResolvedValue([link]);
    const user = userEvent.setup();
    renderApp(`/links/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    return user;
  }

  it('is a number field inside the single config form of a new link, exactly once', async () => {
    await openNew();
    expect(screen.getAllByLabelText(RATE)).toHaveLength(1);
    expect(rate()).toHaveAttribute('type', 'number');
    expect(group()).toContainElement(rate());
  });

  it('sits in the Delayer subsection, after the mean delay and before the Limiter subsection', async () => {
    await openNew();
    const order = (x: HTMLElement, y: HTMLElement) => x.compareDocumentPosition(y) & Node.DOCUMENT_POSITION_FOLLOWING;
    expect(section('Delayer')).toContainElement(rate());
    expect(order(within(group()).getByLabelText('Mean delay'), rate())).toBeTruthy();
    expect(order(rate(), section('Limiter'))).toBeTruthy();
  });

  it('is shown while "Activate correlation" is off and stays after switching it on (always required)', async () => {
    const user = await openNew();
    expect(input(group(), 'Activate correlation')).not.toBeChecked();
    expect(rate()).toBeInTheDocument();
    await user.click(input(group(), 'Activate correlation'));
    expect(rate()).toBeInTheDocument();
  });

  it('a new link starts with the field EMPTY (no preset value); saving the untouched form is refused', async () => {
    const user = await openNew();
    expect(rate()).toHaveValue(null);
    expect(rate()).toHaveDisplayValue('');
    expect(unitOf(group(), RATE)).toBe('Mpps'); // only the unit has a default
    await fillName(user, 'untouched');
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.selectOptions(select('Egress'), '0000:65:00.1');
    await user.click(save()); // everything else valid, only the rate was never entered
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('the shared helper path: typing a rate into the empty form makes it saveable', async () => {
    const user = await openNew();
    await fillValid(user);
    expect(rate()).toHaveValue(Number(TEST_RATE));
    await user.click(save());
    expect(lastCreate().config.delayer.initialization_rate_pps).toBe(Number(TEST_RATE));
  });

  it('sends the typed value as a number', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.clear(rate());
    await user.type(rate(), '3000000');
    await user.click(save());
    const sent = lastCreate().config.delayer.initialization_rate_pps;
    expect(sent).toBe(3000000);
    expect(typeof sent).toBe('number');
  });

  it.each(['1', '1.5', '20000000'])('accepts %s (finite and >= 1)', async (v) => {
    const user = await openNew();
    await fillValid(user);
    await user.clear(rate());
    await user.type(rate(), v);
    await user.click(save());
    expect(lastCreate().config.delayer.initialization_rate_pps).toBe(Number(v));
  });

  it.each(['0', '0.5', '0.999', '-1', '-15000000'])('refuses %s (below 1): alert naming the field, no request', async (v) => {
    const user = await openNew();
    await fillValid(user);
    await user.clear(rate());
    await user.type(rate(), v);
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('refuses a field that was filled and then cleared: alert naming the field, no request, no silent default', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.clear(rate());
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('refuses the empty field also with correlation off, and accepts the corrected value afterwards', async () => {
    const user = await openNew();
    await fillName(user, 'v');
    await user.selectOptions(select('Ingress'), '0000:65:00.0');
    await user.selectOptions(select('Egress'), '0000:65:00.1');
    expect(input(group(), 'Activate correlation')).not.toBeChecked();
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    await typeRate(user, '9000000');
    await user.click(save());
    expect(lastCreate().config.delayer.initialization_rate_pps).toBe(9000000);
  });

  it('the message names the field and does not mention a Direction', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.clear(rate());
    await user.click(save());
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/Initialization rate/);
    expect(alert).not.toHaveTextContent(/Direction \d/);
  });

  it('edit: shows the stored value of the link (8000000 pps as 8 Mpps)', async () => {
    await openEdit(fx.linkRev());
    expect(rate()).toHaveValue(8);
    expect(unitOf(group(), RATE)).toBe('Mpps');
  });

  it('edit: saving without changes sends the stored value back', async () => {
    const user = await openEdit(fx.link({ config: fx.config({ delayer: { ...fx.config().delayer, initialization_rate_pps: 6543210 } }) }));
    expect(rate()).toHaveValue(6543.21); // 5 decimals in Mpps, so kpps
    expect(unitOf(group(), RATE)).toBe('kpps');
    await user.click(save());
    expect(api.updateLink).toHaveBeenCalledTimes(1);
    expect(api.updateLink.mock.calls[0][1].config.delayer.initialization_rate_pps).toBe(6543210);
  });

  it('edit: a changed value is sent instead of the stored one', async () => {
    const user = await openEdit();
    await typeRate(user, '7000000');
    await user.click(save());
    expect(api.updateLink.mock.calls[0][1].config.delayer.initialization_rate_pps).toBe(7000000);
  });

  it('edit: clearing the field is refused, nothing is sent', async () => {
    const user = await openEdit();
    await user.clear(rate());
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(api.updateLink).not.toHaveBeenCalled();
  });

  it('the value of a hairpin link is populated too', async () => {
    await openEdit(fx.loopLink({ config: fx.config3() }));
    expect(rate()).toHaveValue(2.5);
    expect(unitOf(group(), RATE)).toBe('Mpps');
  });
});
