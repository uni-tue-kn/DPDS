import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto, LinkInput } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

const DISTRIBUTIONS = [
  'DeterministicDistribution', 'LogNormalDistribution', 'NormalDistribution', 'GammaDistribution', 'UniformDistribution',
];

// Direction numbering is 0-based like the API and the dashboard (F4): legends read "Direction 0: <rx> → <tx>".
const group = (n: 0 | 1) => screen.getByRole('group', { name: new RegExp(`^Direction ${n}\\b`) });
const select = (name: string) => screen.getByLabelText(name) as HTMLSelectElement;
const input = (scope: HTMLElement, label: string) => within(scope).getByLabelText(label) as HTMLInputElement;

async function openNew(): Promise<UserEvent> {
  const user = userEvent.setup();
  renderApp('/links/new');
  await screen.findByRole('heading', { level: 1, name: 'New link' });
  await waitFor(() => expect(within(select('Port A')).getByRole('option', { name: /0000:65:00\.0/ })).toBeInTheDocument());
  return user;
}

async function fillName(user: UserEvent, name: string) {
  await user.clear(screen.getByLabelText('Name'));
  await user.type(screen.getByLabelText('Name'), name);
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
  for (const v of [c.delayer.mean_ns, c.delayer.std_ns, c.delayer.half_life_period_ns, c.limiter.rate_bps, c.limiter.limit_bit]) {
    expect(typeof v).toBe('number');
    expect(Number.isFinite(v)).toBe(true);
  }
  expect(typeof c.delayer.activate_correlation).toBe('boolean');
  expect(['Spacer', 'TokenBucketFilter']).toContain(c.limiter.mode);
  expect(typeof c.limiter.activate).toBe('boolean');
  expect(['Random', 'GilbertElliot']).toContain(c.loss_model.type);
  expect(typeof c.delay_stats).toBe('boolean');
}

describe('Link editor: interface picker', () => {
  it('lists the interfaces of /api/interfaces in both port selects', async () => {
    await openNew();
    for (const label of ['Port A', 'Port B']) {
      const opts = within(select(label)).getAllByRole('option').map((o) => o.textContent ?? '');
      for (const pci of ['0000:65:00.0', '0000:65:00.1', '0000:b3:00.0', '0000:17:00.0']) {
        expect(opts.some((t) => t.includes(pci))).toBe(true);
      }
    }
  });

  it('shows the netdev name next to the address', async () => {
    await openNew();
    expect(within(select('Port A')).getByRole('option', { name: /enp101s0f0/ })).toBeInTheDocument();
  });

  it('disables unusable and management interfaces and shows the reason', async () => {
    await openNew();
    const a = select('Port A');
    const usable = within(a).getByRole('option', { name: /0000:65:00\.0/ });
    expect(usable).toBeEnabled();
    const unusable = within(a).getByRole('option', { name: /0000:b3:00\.0/ });
    expect(unusable).toBeDisabled();
    expect(unusable).toHaveTextContent('bind to vfio-pci');
    const mgmt = within(a).getByRole('option', { name: /0000:17:00\.0/ });
    expect(mgmt).toBeDisabled();
    expect(mgmt).toHaveTextContent(/management NIC/);
  });

  it('shows an alert if the interfaces cannot be loaded', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/links/new');
    expect(await screen.findByRole('alert')).toHaveTextContent('sysfs unreadable');
  });
});

describe('Link editor: create', () => {
  it('starts with two ports and two direction forms', async () => {
    await openNew();
    expect(select('Number of ports').value).toBe('2');
    expect(select('Port B')).toBeInTheDocument();
    expect(group(0)).toBeInTheDocument();
    expect(group(1)).toBeInTheDocument();
  });

  it('creates a two-port link with valid default configs and navigates to the list', async () => {
    const user = await openNew();
    await fillName(user, 'my-link');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = lastCreate();
    expect(sent.name).toBe('my-link');
    expect(sent.ports).toEqual(['0000:65:00.0', '0000:65:00.1']);
    expect(sent.configs).toHaveLength(2);
    sent.configs.forEach(expectValidConfig);
    expect(sent.configs[0].delay_stats).toBe(true);
    expect(sent.configs[0].reordering).toBe(false);
    expect(sent.configs[0].loss_model.type).toBe('Random');
    expect(sent.workers ?? null).toBeNull();
    expect(sent.configs[0].seed ?? null).toBeNull();
    expect(sent.configs[0].histogram?.bins ?? null).toBeNull();
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('1 port: loopback with a single direction form', async () => {
    const user = await openNew();
    await user.selectOptions(select('Number of ports'), '1');
    expect(screen.queryByLabelText('Port B')).not.toBeInTheDocument();
    expect(screen.queryByRole('group', { name: /^Direction 1\b/ })).not.toBeInTheDocument();
    expect(group(0)).toBeInTheDocument();
    await fillName(user, 'loop');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = lastCreate();
    expect(sent.ports).toEqual(['0000:65:00.0']);
    expect(sent.configs).toHaveLength(1);
  });

  it('switching back from 1 to 2 ports restores the second form (valid config)', async () => {
    const user = await openNew();
    await user.selectOptions(select('Number of ports'), '1');
    await user.selectOptions(select('Number of ports'), '2');
    expect(group(1)).toBeInTheDocument();
    await fillName(user, 'x');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = lastCreate();
    expect(sent.configs).toHaveLength(2);
    sent.configs.forEach(expectValidConfig);
  });

  it('sends every config field as typed, per direction (direction 2 independent of direction 1)', async () => {
    const user = await openNew();
    await fillName(user, 'full');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');

    const g1 = group(0);
    await user.selectOptions(within(g1).getByLabelText('Delay distribution'), 'GammaDistribution');
    await user.clear(input(g1, 'Mean delay (ns)'));
    await user.type(input(g1, 'Mean delay (ns)'), '2500000');
    await user.clear(input(g1, 'Delay std (ns)'));
    await user.type(input(g1, 'Delay std (ns)'), '120000');
    await user.click(input(g1, 'Activate correlation'));
    await user.clear(input(g1, 'Half-life period (ns)'));
    await user.type(input(g1, 'Half-life period (ns)'), '8000000');
    await user.click(input(g1, 'Limiter active'));
    await user.selectOptions(within(g1).getByLabelText('Limiter mode'), 'TokenBucketFilter');
    await user.clear(input(g1, 'Rate (bit/s)'));
    await user.type(input(g1, 'Rate (bit/s)'), '100000000000');
    await user.clear(input(g1, 'Burst limit (bit)'));
    await user.type(input(g1, 'Burst limit (bit)'), '240000');
    await user.clear(input(g1, 'Buffer capacity'));
    await user.type(input(g1, 'Buffer capacity'), '250000');
    await user.click(input(g1, 'Reordering'));
    await user.clear(input(g1, 'Seed'));
    await user.type(input(g1, 'Seed'), '1234');
    await user.type(input(g1, 'Histogram bins'), '128');
    await user.type(input(g1, 'Histogram lo (ns)'), '1000000');
    await user.type(input(g1, 'Histogram hi (ns)'), '4000000');
    await user.click(input(g1, 'Delay statistics')); // default on -> off

    // random loss
    await user.clear(input(g1, 'Random loss probability'));
    await user.type(input(g1, 'Random loss probability'), '0.05');

    const g2 = group(1);
    await user.selectOptions(within(g2).getByLabelText('Loss model'), 'GilbertElliot');
    await user.clear(input(g2, 'Gilbert-Elliot p'));
    await user.type(input(g2, 'Gilbert-Elliot p'), '0.01');
    await user.clear(input(g2, 'Gilbert-Elliot r'));
    await user.type(input(g2, 'Gilbert-Elliot r'), '0.2');
    await user.clear(input(g2, 'Gilbert-Elliot k'));
    await user.type(input(g2, 'Gilbert-Elliot k'), '0.9');
    await user.clear(input(g2, 'Gilbert-Elliot h'));
    await user.type(input(g2, 'Gilbert-Elliot h'), '0.3');
    await user.click(input(g2, 'Gilbert-Elliot starts in good state')); // default on -> off

    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const [c1, c2] = lastCreate().configs;
    expect(c1.delayer).toEqual({
      distribution: 'GammaDistribution', mean_ns: 2500000, std_ns: 120000,
      activate_correlation: true, half_life_period_ns: 8000000,
    });
    expect(c1.limiter).toEqual({ mode: 'TokenBucketFilter', activate: true, rate_bps: 100000000000, limit_bit: 240000 });
    expect(c1.buffer_capacity).toBe(250000);
    expect(c1.reordering).toBe(true);
    expect(c1.seed).toBe(1234);
    expect(c1.histogram).toEqual({ bins: 128, lo_ns: 1000000, hi_ns: 4000000 });
    expect(c1.delay_stats).toBe(false);
    expect(c1.loss_model).toEqual({ type: 'Random', p: 0.05 });
    // direction 2 untouched except for the loss model
    expect(c2.delay_stats).toBe(true);
    expect(c2.reordering).toBe(false);
    expect(c2.loss_model).toEqual({ type: 'GilbertElliot', p: 0.01, r: 0.2, k: 0.9, h: 0.3, state_is_good: false });
    expect(c2.limiter.activate).toBe(false);
  });

  it('shows only the fields of the selected loss model', async () => {
    const user = await openNew();
    const g1 = group(0);
    expect(within(g1).getByLabelText('Random loss probability')).toBeInTheDocument();
    expect(within(g1).queryByLabelText('Gilbert-Elliot p')).not.toBeInTheDocument();
    await user.selectOptions(within(g1).getByLabelText('Loss model'), 'GilbertElliot');
    expect(within(g1).queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    for (const n of ['p', 'r', 'k', 'h']) expect(within(g1).getByLabelText(`Gilbert-Elliot ${n}`)).toBeInTheDocument();
    expect(within(g1).getByLabelText('Gilbert-Elliot starts in good state')).toBeChecked();
  });

  it('offers exactly the five delay distributions and the two limiter modes', async () => {
    await openNew();
    const g1 = group(0);
    const dist = within(within(g1).getByLabelText('Delay distribution') as HTMLElement).getAllByRole('option');
    expect(dist.map((o) => (o as HTMLOptionElement).value).sort()).toEqual([...DISTRIBUTIONS].sort());
    const modes = within(within(g1).getByLabelText('Limiter mode') as HTMLElement).getAllByRole('option');
    expect(modes.map((o) => (o as HTMLOptionElement).value).sort()).toEqual(['Spacer', 'TokenBucketFilter']);
    const loss = within(within(g1).getByLabelText('Loss model') as HTMLElement).getAllByRole('option');
    expect(loss.map((o) => (o as HTMLOptionElement).value).sort()).toEqual(['GilbertElliot', 'Random']);
  });

  it('only some histogram fields filled: the others are null/absent', async () => {
    const user = await openNew();
    await fillName(user, 'h');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(input(group(0), 'Histogram bins'), '64');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const h = lastCreate().configs[0].histogram;
    expect(h?.bins).toBe(64);
    expect(h?.lo_ns ?? null).toBeNull();
    expect(h?.hi_ns ?? null).toBeNull();
  });

  it('worker override: comma separated lcores in direction order', async () => {
    const user = await openNew();
    await fillName(user, 'w');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(screen.getByLabelText('Worker lcores'), '7, 2');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(lastCreate().workers).toEqual([7, 2]);
  });

  it('an empty worker override means automatic (null/absent)', async () => {
    const user = await openNew();
    expect(screen.getByLabelText('Worker lcores')).toHaveValue('');
    await fillName(user, 'w');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(lastCreate().workers ?? null).toBeNull();
  });

  it('does not call the API without a name', async () => {
    const user = await openNew();
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.createLink).not.toHaveBeenCalled();
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
  });

  it('does not call the API without selected ports', async () => {
    const user = await openNew();
    await fillName(user, 'noports');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.createLink).not.toHaveBeenCalled();
  });

  it('shows the API validation error, stays on the page and keeps the form values', async () => {
    api.createLink.mockRejectedValue(new ApiError(400, 'direction 1: delayer.std_ns must be finite and >= 0'));
    const user = await openNew();
    await fillName(user, 'bad');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.clear(input(group(0), 'Buffer capacity'));
    await user.type(input(group(0), 'Buffer capacity'), '777');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(await screen.findByRole('alert')).toHaveTextContent('direction 1: delayer.std_ns must be finite and >= 0');
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue('bad');
    expect(input(group(0), 'Buffer capacity')).toHaveValue(777);
    // the form can be corrected and resubmitted
    api.createLink.mockImplementation(async (i) => ({ id: 3, ...i }));
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.createLink).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('shows a 409 conflict message (port already used)', async () => {
    api.createLink.mockRejectedValue(new ApiError(409, 'port 0000:65:00.0 is already used by link "other"'));
    const user = await openNew();
    await fillName(user, 'dup');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/already used by link "other"/);
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
    await fillName(user, 'once');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    const save = screen.getByRole('button', { name: /^save$/i });
    await user.click(save);
    await user.click(save);
    expect(api.createLink).toHaveBeenCalledTimes(1);
  });
});

describe('Link editor: edit', () => {
  async function openEdit(link = fx.link()) {
    api.getLink.mockResolvedValue(link);
    api.getLinks.mockResolvedValue([link]);
    const user = userEvent.setup();
    renderApp(`/links/${link.id}`);
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue(link.name));
    return user;
  }

  it('populates name, ports and the worker override', async () => {
    await openEdit(fx.link({ workers: [7, 2] }));
    expect(select('Number of ports').value).toBe('2');
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    expect(select('Port B').value).toBe('0000:65:00.1');
    expect(screen.getByLabelText('Worker lcores')).toHaveValue('7, 2');
  });

  it('populates every field of direction 1', async () => {
    await openEdit();
    const g = group(0);
    expect(within(g).getByLabelText('Delay distribution')).toHaveValue('NormalDistribution');
    expect(input(g, 'Mean delay (ns)')).toHaveValue(1000);
    expect(input(g, 'Delay std (ns)')).toHaveValue(10);
    expect(input(g, 'Activate correlation')).not.toBeChecked();
    expect(input(g, 'Half-life period (ns)')).toHaveValue(0);
    expect(input(g, 'Limiter active')).not.toBeChecked();
    expect(within(g).getByLabelText('Limiter mode')).toHaveValue('Spacer');
    expect(input(g, 'Rate (bit/s)')).toHaveValue(1e9);
    expect(input(g, 'Burst limit (bit)')).toHaveValue(0);
    expect(within(g).getByLabelText('Loss model')).toHaveValue('Random');
    expect(input(g, 'Random loss probability')).toHaveValue(0);
    expect(input(g, 'Buffer capacity')).toHaveValue(1000);
    expect(input(g, 'Reordering')).not.toBeChecked();
    expect(input(g, 'Seed')).toHaveDisplayValue('');
    expect(input(g, 'Histogram bins')).toHaveValue(null);
    expect(input(g, 'Histogram lo (ns)')).toHaveValue(null);
    expect(input(g, 'Histogram hi (ns)')).toHaveValue(null);
    expect(input(g, 'Delay statistics')).toBeChecked();
  });

  it('populates every field of direction 2 (Gilbert-Elliot, TBF, histogram, delay_stats off)', async () => {
    await openEdit();
    const g = group(1);
    expect(within(g).getByLabelText('Delay distribution')).toHaveValue('GammaDistribution');
    expect(input(g, 'Mean delay (ns)')).toHaveValue(2500);
    expect(input(g, 'Delay std (ns)')).toHaveValue(300);
    expect(input(g, 'Activate correlation')).toBeChecked();
    expect(input(g, 'Half-life period (ns)')).toHaveValue(4000000);
    expect(input(g, 'Limiter active')).toBeChecked();
    expect(within(g).getByLabelText('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(input(g, 'Rate (bit/s)')).toHaveValue(40000000000);
    expect(input(g, 'Burst limit (bit)')).toHaveValue(120000);
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
    expect(input(g, 'Histogram lo (ns)')).toHaveValue(100);
    expect(input(g, 'Histogram hi (ns)')).toHaveValue(9000);
    expect(input(g, 'Delay statistics')).not.toBeChecked();
  });

  it('saving without changes sends the same values back via updateLink(id, ...)', async () => {
    const link = fx.link();
    const user = await openEdit(link);
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.createLink).not.toHaveBeenCalled();
    expect(api.updateLink).toHaveBeenCalledTimes(1);
    const [id, sent] = api.updateLink.mock.calls[0];
    expect(id).toBe(1);
    expect(sent.name).toBe('a-b');
    expect(sent.ports).toEqual(link.ports);
    expect(sent.configs[0].delayer).toEqual(link.configs[0].delayer);
    expect(sent.configs[0].limiter).toEqual(link.configs[0].limiter);
    expect(sent.configs[0].loss_model).toEqual(link.configs[0].loss_model);
    expect(sent.configs[0].buffer_capacity).toBe(1000);
    expect(sent.configs[1].delayer).toEqual(link.configs[1].delayer);
    expect(sent.configs[1].limiter).toEqual(link.configs[1].limiter);
    expect(sent.configs[1].loss_model).toEqual(link.configs[1].loss_model);
    expect(sent.configs[1].seed).toBe(42);
    expect(sent.configs[1].histogram).toEqual({ bins: 64, lo_ns: 100, hi_ns: 9000 });
    expect(sent.configs[1].delay_stats).toBe(false);
    expect(sent.configs[1].reordering).toBe(true);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('clearing the seed sends null; clearing the histogram fields sends no histogram values', async () => {
    const user = await openEdit();
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    const g2 = group(1);
    await user.clear(input(g2, 'Seed'));
    await user.clear(input(g2, 'Histogram bins'));
    await user.clear(input(g2, 'Histogram lo (ns)'));
    await user.clear(input(g2, 'Histogram hi (ns)'));
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const c2 = api.updateLink.mock.calls[0][1].configs[1];
    expect(c2.seed ?? null).toBeNull();
    expect(c2.histogram?.bins ?? null).toBeNull();
    expect(c2.histogram?.lo_ns ?? null).toBeNull();
    expect(c2.histogram?.hi_ns ?? null).toBeNull();
  });

  it('a stored non-interface port (vdev) stays selected and is sent back unchanged', async () => {
    const loop = fx.loopLink({ id: 5 });
    const user = await openEdit(loop);
    expect(select('Number of ports').value).toBe('1');
    expect(select('Port A').value).toBe('net_null0');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(api.updateLink.mock.calls[0][1].ports).toEqual(['net_null0']);
    expect(api.updateLink.mock.calls[0][0]).toBe(5);
  });

  it('removing the second direction (2 -> 1 port) sends one config', async () => {
    const user = await openEdit();
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    await user.selectOptions(select('Number of ports'), '1');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    const sent = api.updateLink.mock.calls[0][1];
    expect(sent.ports).toEqual(['0000:65:00.0']);
    expect(sent.configs).toHaveLength(1);
    expect(sent.configs[0].buffer_capacity).toBe(1000);
  });

  it('shows API errors of update (404 link vanished)', async () => {
    api.updateLink.mockRejectedValue(new ApiError(404, 'link 1 does not exist'));
    const user = await openEdit();
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    expect(await screen.findByRole('alert')).toHaveTextContent('link 1 does not exist');
  });

  it('shows an alert if the link cannot be loaded', async () => {
    api.getLink.mockRejectedValue(new ApiError(404, 'link 9 does not exist'));
    renderApp('/links/9');
    expect(await screen.findByRole('alert')).toHaveTextContent('link 9 does not exist');
  });
});

describe('Link editor: typed ports (F5)', () => {
  // Contract: next to each port picker (<select> "Port A"/"Port B") there is a free-text input labelled
  // "Port A (typed)" / "Port B (typed)". A non-empty typed value wins over the picker; empty -> the picker value.
  const typed = (p: 'A' | 'B') => screen.getByLabelText(`Port ${p} (typed)`) as HTMLInputElement;
  const save = () => screen.getByRole('button', { name: /^save$/i });

  async function fillValid(user: UserEvent) {
    await fillName(user, 'typed');
  }

  it('offers a typed field next to each picker, and none for Port B in loopback mode', async () => {
    const user = await openNew();
    expect(typed('A')).toHaveValue('');
    expect(typed('B')).toHaveValue('');
    await user.selectOptions(select('Number of ports'), '1');
    expect(screen.getByLabelText('Port A (typed)')).toBeInTheDocument();
    expect(screen.queryByLabelText('Port B (typed)')).not.toBeInTheDocument();
  });

  it.each(['0000:af:00.0', '0000:65:00.7', 'net_null0', 'net_null12', 'net_ring1', 'net_ring007'])(
    'accepts the typed port %s and sends it unchanged',
    async (port) => {
      const user = await openNew();
      await fillValid(user);
      await user.type(typed('A'), port);
      await user.selectOptions(select('Port B'), '0000:65:00.1');
      await user.click(save());
      expect(lastCreate().ports).toEqual([port, '0000:65:00.1']);
    },
  );

  it('accepts typed values in both fields (vdev pair)', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.type(typed('A'), 'net_null0');
    await user.type(typed('B'), 'net_ring1');
    await user.click(save());
    expect(lastCreate().ports).toEqual(['net_null0', 'net_ring1']);
    expect(await screen.findByRole('heading', { level: 1, name: 'Links' })).toBeInTheDocument();
  });

  it('accepts a typed loopback port', async () => {
    const user = await openNew();
    await user.selectOptions(select('Number of ports'), '1');
    await fillValid(user);
    await user.type(typed('A'), 'net_null0');
    await user.click(save());
    const sent = lastCreate();
    expect(sent.ports).toEqual(['net_null0']);
    expect(sent.configs).toHaveLength(1);
  });

  it('a typed value wins over the picker; clearing it falls back to the picker', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(typed('A'), 'net_null0');
    await user.clear(typed('A'));
    await user.click(save());
    expect(lastCreate().ports).toEqual(['0000:65:00.0', '0000:65:00.1']);
  });

  it('a typed value replaces the picker choice', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(typed('B'), 'net_ring3');
    await user.click(save());
    expect(lastCreate().ports).toEqual(['0000:65:00.0', 'net_ring3']);
  });

  it.each([
    'net_pcap0,tx_pcap=/x',
    'net_pcap0',
    'net_null',
    'net_ring',
    'net_null0,size=64',
    'net_ring1,nodeaction=x:0:CREATE',
    'net_nullx',
    'net_tap0',
    'xnet_null0',
    'net_null0x',
    'eth0',
    'enp101s0f0',
    '0000:65:00',
    '0000:65:00.8',
    '000:65:00.0',
    '0000:65:00.00',
    '0000:65:0g.0',
    '/etc/passwd',
    'net_null0;reboot',
  ])('rejects the typed port %s: message, no request', async (port) => {
    const user = await openNew();
    await fillValid(user);
    await user.type(typed('A'), port);
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/port/i);
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue('typed');
  });

  it('rejects an invalid typed Port B in a two-port link, accepts it once fixed', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.type(typed('B'), 'net_pcap0,rx_pcap=/tmp/in.pcap');
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/port/i);
    await user.clear(typed('B'));
    await user.type(typed('B'), 'net_null1');
    await user.click(save());
    expect(lastCreate().ports).toEqual(['0000:65:00.0', 'net_null1']);
  });

  it('an invalid typed Port B is ignored in loopback mode (only one port is sent)', async () => {
    const user = await openNew();
    await fillValid(user);
    await user.type(typed('B'), 'garbage');
    await user.selectOptions(select('Number of ports'), '1');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.click(save());
    expect(lastCreate().ports).toEqual(['0000:65:00.0']);
  });

  it('editing a link whose port is a vdev: the typed field stays empty, the port is kept', async () => {
    api.getLink.mockResolvedValue(fx.loopLink({ id: 5 }));
    renderApp('/links/5');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(select('Port A').value).toBe('net_null0'));
    expect(typed('A')).toHaveValue('');
  });
});

describe('Link editor: seeds beyond 2^53-1 (F5)', () => {
  const save = () => screen.getByRole('button', { name: /^save$/i });

  async function withSeed(seed: string) {
    const user = await openNew();
    await fillName(user, 's');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(input(group(0), 'Seed'), seed);
    await user.click(save());
    return user;
  }

  it('sends the largest safe seed 9007199254740991 exactly', async () => {
    await withSeed('9007199254740991');
    expect(lastCreate().configs[0].seed).toBe(9007199254740991);
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

  it('names the direction (0-based) of the offending seed', async () => {
    const user = await openNew();
    await fillName(user, 's');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.type(input(group(1), 'Seed'), '9007199254740992');
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Direction 1\b.*seed/i);
  });

  it('seed 0 and a small seed are still fine', async () => {
    await withSeed('0');
    expect(lastCreate().configs[0].seed).toBe(0);
  });

  it('a refused seed can be corrected and saved', async () => {
    const user = await withSeed('9007199254740992');
    await screen.findByRole('alert');
    await user.clear(input(group(0), 'Seed'));
    await user.type(input(group(0), 'Seed'), '42');
    await user.click(save());
    expect(lastCreate().configs[0].seed).toBe(42);
  });
});

describe('Link editor: direction labels (F4)', () => {
  const save = () => screen.getByRole('button', { name: /^save$/i });

  it('legends are 0-based "Direction d: rx -> tx" and follow the chosen ports', async () => {
    const user = await openNew();
    expect(group(0)).toBeInTheDocument();
    expect(group(1)).toBeInTheDocument();
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    expect(screen.getByRole('group', { name: /^Direction 0: 0000:65:00\.0 → 0000:65:00\.1/ })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /^Direction 1: 0000:65:00\.1 → 0000:65:00\.0/ })).toBeInTheDocument();
  });

  it('typed ports show up in the legends', async () => {
    const user = await openNew();
    await user.type(screen.getByLabelText('Port A (typed)'), 'net_null0');
    await user.type(screen.getByLabelText('Port B (typed)'), 'net_ring1');
    expect(screen.getByRole('group', { name: /^Direction 0: net_null0 → net_ring1/ })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /^Direction 1: net_ring1 → net_null0/ })).toBeInTheDocument();
  });

  it('the editor legend matches the dashboard card label for the stored link (same text)', async () => {
    api.getLink.mockResolvedValue(fx.link());
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(select('Port A').value).toBe('0000:65:00.0'));
    expect(screen.getByRole('group', { name: /^Direction 0: 0000:65:00\.0 → 0000:65:00\.1/ })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: /^Direction 1: 0000:65:00\.1 → 0000:65:00\.0/ })).toBeInTheDocument();
  });

  it('a loopback link has one legend "Direction 0: p -> p"', async () => {
    api.getLink.mockResolvedValue(fx.loopLink({ id: 5 }));
    renderApp('/links/5');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(select('Port A').value).toBe('net_null0'));
    expect(screen.getByRole('group', { name: /^Direction 0: net_null0 → net_null0/ })).toBeInTheDocument();
    expect(screen.queryByRole('group', { name: /^Direction 1\b/ })).not.toBeInTheDocument();
  });

  it('validation messages use the same 0-based numbering (first direction)', async () => {
    const user = await openNew();
    await fillName(user, 'v');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.clear(input(group(0), 'Buffer capacity'));
    await user.click(save());
    expect(api.createLink).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent(/Direction 0: Buffer capacity/);
  });

  it('validation messages use the same 0-based numbering (second direction)', async () => {
    const user = await openNew();
    await fillName(user, 'v');
    await user.selectOptions(select('Port A'), '0000:65:00.0');
    await user.selectOptions(select('Port B'), '0000:65:00.1');
    await user.clear(input(group(1), 'Mean delay (ns)'));
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent(/Direction 1: Mean delay \(ns\)/);
  });

  it('all element ids on the page are unique (no label-derived control ids colliding)', async () => {
    await openNew();
    const ids = Array.from(document.querySelectorAll('[id]')).map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
    // and every labelled port control still resolves from its label
    for (const l of ['Port A', 'Port B', 'Port A (typed)', 'Port B (typed)']) expect(screen.getByLabelText(l)).toBeInTheDocument();
  });
});
