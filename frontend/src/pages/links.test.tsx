import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { deferred, field, renderApp } from '../test/helpers';

// v16 (supersedes the v12 "PCI (netdev)" labels, see the v16 block of src/test/helpers.tsx): the Links list has the
// columns Name | Interfaces | Delay | Reordering | Limiter | Loss | actions; an interface is shown by its netdev, the device name when it has none.
const IFACES = 'enp101s0f0 → enp101s0f1';
const REV_IFACES = 'enp101s0f1 → enp101s0f0';
const PLAIN_IFACES = '0000:65:00.0 → 0000:65:00.1';

/** The `<tr>` of the link called `name` (the name cell's own text is exactly the name). */
const rowOf = async (name: string) => (await screen.findByText(name)).closest('tr') as HTMLElement;

describe('Links list', () => {
  it('has the columns Name, Interfaces, Delay, Reordering, Limiter and Loss plus one action column', async () => {
    renderApp('/links');
    await screen.findByText('a-b');
    const headers = screen.getAllByRole('columnheader').map((h) => h.textContent?.trim());
    expect(headers).toHaveLength(7);
    expect(headers.slice(0, 6)).toEqual(['Name', 'Interfaces', 'Delay', 'Reordering', 'Limiter', 'Loss']);
    expect(screen.queryByRole('columnheader', { name: /worker|configuration|summary/i })).not.toBeInTheDocument();
  });

  // spec19 item 7: the empty header of the action column gets a visually hidden "Actions" (screen readers, Bootstrap's
  // `.visually-hidden`: the text exists but is not painted).
  it('the action column header carries a visually hidden "Actions" text', async () => {
    renderApp('/links');
    await screen.findByText('a-b');
    const headers = screen.getAllByRole('columnheader');
    expect(headers).toHaveLength(7);
    const last = headers[6];
    expect(last).toHaveAccessibleName('Actions');
    const hidden = within(last).getByText('Actions');
    expect(hidden.classList.contains('visually-hidden')).toBe(true);
    // the other six headers are untouched
    expect(headers.slice(0, 6).map((h) => h.textContent?.trim())).toEqual(['Name', 'Interfaces', 'Delay', 'Reordering', 'Limiter', 'Loss']);
    expect(screen.getAllByRole('columnheader', { name: 'Actions' })).toHaveLength(1);
  });

  it('lists one row per link: the name cell holds the name only, the interface cell "ingress → egress" by netdev', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev(), fx.loopLink()]);
    renderApp('/links');
    const row = await rowOf('a-b');
    expect(within(row).getAllByRole('cell')[0].textContent).toBe('a-b');
    expect(within(row).getAllByRole('cell')[1].textContent).toBe(IFACES);
    const rev = await rowOf('b-a');
    expect(rev).not.toBe(row);
    expect(within(rev).getAllByRole('cell')[1].textContent).toBe(REV_IFACES);
    const loop = await rowOf('loop');
    expect(within(loop).getAllByRole('cell')[1].textContent).toBe('net_null0 → net_null0');
    expect(screen.getAllByRole('row')).toHaveLength(1 + 3); // header + one row per link
  });

  it('the interface cell is one element whose own text is the whole "ingress → egress"', async () => {
    renderApp('/links');
    const el = await screen.findByText(IFACES);
    expect(el.textContent).toBe(IFACES);
    expect(el.closest('tr')).toBe(await rowOf('a-b'));
  });

  it('no PCI address, no parentheses, no square brackets and no worker in the visible row text', async () => {
    renderApp('/links');
    const row = await rowOf('a-b');
    expect(row.textContent).not.toMatch(/0000:/);
    expect(row.textContent).not.toMatch(/[()[\]]/);
    expect(row.textContent).not.toMatch(/auto|worker/i);
  });

  it('the old "a ↔ b" pair notation and the direction count are gone', async () => {
    renderApp('/links');
    const row = await rowOf('a-b');
    expect(row).not.toHaveTextContent('↔');
    expect(screen.queryByRole('columnheader', { name: /directions/i })).not.toBeInTheDocument();
  });

  it('shows an empty state without links', async () => {
    api.getLinks.mockResolvedValue([]);
    renderApp('/links');
    expect(await screen.findByText(/no links/i)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /new link/i })).toBeInTheDocument();
  });

  it('shows the API error if the list cannot be loaded', async () => {
    api.getLinks.mockRejectedValue(new ApiError(500, 'state file is corrupt'));
    renderApp('/links');
    expect(await screen.findByRole('alert')).toHaveTextContent('state file is corrupt');
  });

  it('"New link" opens the empty editor', async () => {
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('link', { name: /new link/i }));
    expect(await screen.findByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
  });

  it('"Edit <name>" opens the editor of that link', async () => {
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('link', { name: 'Edit a-b' }));
    expect(await screen.findByRole('heading', { level: 1, name: 'Edit link' })).toBeInTheDocument();
    expect(api.getLink).toHaveBeenCalledWith(1);
  });

  it('every row keeps its Edit link, Delete button and Inspect button, named per link', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev()]);
    renderApp('/links');
    await screen.findByText('a-b');
    for (const n of ['a-b', 'b-a']) {
      const row = await rowOf(n);
      expect(within(row).getByRole('link', { name: `Edit ${n}` })).toBeInTheDocument();
      expect(within(row).getByRole('button', { name: `Delete ${n}` })).toBeInTheDocument();
      expect(within(row).getByRole('button', { name: `Inspect ${n}` })).toBeInTheDocument();
    }
  });

  it('shows the pending-restart notice only when the emulation says so', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running', { pending_restart: true }));
    const view = renderApp('/links');
    expect(await screen.findByTestId('pending-restart')).toHaveTextContent(/restart/i);
    view.unmount();
    api.getEmulation.mockClear();
    api.getEmulation.mockResolvedValue(fx.emulation('running', { pending_restart: false }));
    renderApp('/links');
    await screen.findByText('a-b');
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByTestId('pending-restart')).not.toBeInTheDocument();
  });
});

describe('Links delete', () => {
  it('asks for confirmation; Cancel does not delete', async () => {
    const user = userEvent.setup();
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Delete a-b' }));
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('a-b');
    expect(api.deleteLink).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(api.deleteLink).not.toHaveBeenCalled();
  });

  it('Confirm deletes by id and reloads the list', async () => {
    const user = userEvent.setup();
    api.getLinks.mockResolvedValueOnce([fx.link(), fx.loopLink()]).mockResolvedValue([fx.loopLink()]);
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Delete a-b' }));
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Confirm delete' }));
    expect(api.deleteLink).toHaveBeenCalledTimes(1);
    expect(api.deleteLink).toHaveBeenCalledWith(1);
    await waitFor(() => expect(screen.queryByText('a-b')).not.toBeInTheDocument());
    expect(screen.getByText('loop')).toBeInTheDocument();
  });

  it('shows the API error when deleting fails', async () => {
    const user = userEvent.setup();
    api.deleteLink.mockRejectedValue(new ApiError(404, 'link 1 does not exist'));
    renderApp('/links');
    await user.click(await screen.findByRole('button', { name: 'Delete a-b' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    expect(await screen.findByText(/link 1 does not exist/)).toBeInTheDocument();
  });
});

describe('Links list: interfaces are shown by netdev, device name as fallback (v16)', () => {
  const cellText = async (name: string) => within(await rowOf(name)).getAllByRole('cell')[1].textContent;

  it('a hairpin on a real NIC shows the netdev twice', async () => {
    api.getLinks.mockResolvedValue([fx.link({ id: 5, name: 'hair', ingress: '0000:65:00.1', egress: '0000:65:00.1' })]);
    renderApp('/links');
    expect(await screen.findByText('enp101s0f1 → enp101s0f1')).toBeInTheDocument();
  });

  it('a port whose interface has no netdev (vfio-bound) shows its PCI address', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({ pci: '0000:65:00.0', netdev: null }),
      fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' }),
    ]);
    renderApp('/links');
    expect(await screen.findByText('0000:65:00.0 → enp101s0f1')).toBeInTheDocument();
  });

  it('a port that is not in the interface list shows its device name', async () => {
    api.getInterfaces.mockResolvedValue([fx.iface({ pci: '0000:65:00.0', netdev: 'enp101s0f0' })]);
    renderApp('/links');
    expect(await screen.findByText('enp101s0f0 → 0000:65:00.1')).toBeInTheDocument();
  });

  it('a vdev next to a NIC: only the NIC is shown by netdev', async () => {
    api.getLinks.mockResolvedValue([fx.link({ id: 6, name: 'mix', ingress: 'net_null0', egress: '0000:65:00.0' })]);
    renderApp('/links');
    expect(await screen.findByText('net_null0 → enp101s0f0')).toBeInTheDocument();
  });

  it('an empty interface list leaves device names only', async () => {
    api.getInterfaces.mockResolvedValue([]);
    renderApp('/links');
    expect(await screen.findByText(PLAIN_IFACES)).toBeInTheDocument();
  });

  it('a failing getInterfaces falls back to device names without any alert', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/links');
    expect(await screen.findByText(PLAIN_IFACES)).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/sysfs unreadable/);
    expect(screen.getByRole('link', { name: 'Edit a-b' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete a-b' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Inspect a-b' })).toBeInTheDocument();
  });

  it('a failing getLinks still shows its own error (the interfaces fallback does not swallow it)', async () => {
    api.getLinks.mockRejectedValue(new ApiError(500, 'state file is corrupt'));
    renderApp('/links');
    expect(await screen.findByRole('alert')).toHaveTextContent('state file is corrupt');
  });

  it('a slow interface list does not delay the rows: they show plain names first, then the netdevs', async () => {
    const d = deferred<ReturnType<typeof fx.interfaces>>();
    api.getInterfaces.mockReturnValue(d.promise);
    renderApp('/links');
    expect(await screen.findByText(PLAIN_IFACES)).toBeInTheDocument();
    d.resolve(fx.interfaces());
    expect(await screen.findByText(IFACES)).toBeInTheDocument();
    expect(screen.queryByText(PLAIN_IFACES)).not.toBeInTheDocument();
    expect(await cellText('a-b')).toBe(IFACES);
  });

  it('asks for the interfaces once per page load, also after a delete reloads the list', async () => {
    const user = userEvent.setup();
    api.getLinks.mockResolvedValueOnce([fx.link(), fx.loopLink()]).mockResolvedValue([fx.loopLink()]);
    renderApp('/links');
    await screen.findByText(IFACES);
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Delete a-b' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(screen.queryByText('a-b')).not.toBeInTheDocument());
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
  });
});

// v16 pick: the inspect view is a SIDE PANEL (react-bootstrap Offcanvas, the user's "slider"), not an expandable row.
//   - per row a button named "Inspect <link name>"; clicking it opens ONE `role="dialog"` whose accessible name is the
//     link name (heading inside), with a button "Close" and a link "Edit <link name>" (-> /links/<id>);
//   - closed: no dialog in the document;
//   - the dialog holds <dt>/<dd> pairs (helpers `field`): Ingress, Egress (dd contains the netdev AND the PCI address,
//     a device name without netdev stands alone), Worker lcore ("auto" or the number), outside any subsection;
//   - five `role="group"` subsections named Buffer, Delayer, Limiter, Dropper, Statistics in this order, each with
//     <dt>/<dd> pairs. Values: numbers plain, "<number> <unit>" for unit fields (editor display rule), on/off as "on"/"off".
describe('Links: inspect panel (v16)', () => {
  async function inspect(name: string, links = [fx.link(), fx.linkRev(), fx.loopLink()]) {
    api.getLinks.mockResolvedValue(links);
    const user = userEvent.setup();
    renderApp('/links');
    await screen.findByText(name);
    await user.click(screen.getByRole('button', { name: `Inspect ${name}` }));
    const dialog = await screen.findByRole('dialog', { name });
    return { user, dialog };
  }
  const group = (dialog: HTMLElement, name: string) => within(dialog).getByRole('group', { name });
  const dts = (g: HTMLElement) => Array.from(g.querySelectorAll('dt')).map((d) => d.textContent?.trim());

  // spec19 item 7: the panel names the distribution like the list's Delay cell ("Normal", never "NormalDistribution").
  it.each([
    ['DeterministicDistribution', 'Deterministic'],
    ['NormalDistribution', 'Normal'],
    ['LogNormalDistribution', 'LogNormal'],
    ['GammaDistribution', 'Gamma'],
    ['UniformDistribution', 'Uniform'],
  ] as const)('%s is shown as "%s" in the panel, the same word the list uses', async (distribution, short) => {
    const base = fx.config();
    const link = fx.link({ config: { ...base, delayer: { ...base.delayer, distribution } } });
    const { dialog } = await inspect('a-b', [link]);
    const shown = field(group(dialog, 'Delayer'), 'Delay distribution').textContent?.trim();
    expect(shown).toBe(short);
    expect(dialog.textContent).not.toContain('Distribution');
    // the list row behind the panel starts its Delay cell with the same word
    const row = screen.getAllByText('a-b')[0].closest('tr') as HTMLElement;
    expect(within(row).getAllByRole('cell')[2].textContent?.startsWith(short)).toBe(true);
  });

  it('nothing is open before the button is clicked', async () => {
    renderApp('/links');
    await screen.findByText('a-b');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('the Inspect button opens exactly one dialog named after the link, with a heading of the name', async () => {
    const { dialog } = await inspect('b-a');
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(within(dialog).getByRole('heading', { name: 'b-a' })).toBeInTheDocument();
  });

  it('the Close button closes it', async () => {
    const { user, dialog } = await inspect('a-b');
    await user.click(within(dialog).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });

  it('shows the ports with netdev AND PCI address, and the worker', async () => {
    const { dialog } = await inspect('a-b', [fx.link({ worker: 7 })]);
    expect(field(dialog, 'Ingress')).toHaveTextContent('enp101s0f0');
    expect(field(dialog, 'Ingress')).toHaveTextContent('0000:65:00.0');
    expect(field(dialog, 'Egress')).toHaveTextContent('enp101s0f1');
    expect(field(dialog, 'Egress')).toHaveTextContent('0000:65:00.1');
    expect(field(dialog, 'Worker lcore').textContent?.trim()).toBe('7');
    for (const g of ['Buffer', 'Delayer', 'Limiter', 'Dropper', 'Statistics']) {
      expect(group(dialog, g)).not.toContainElement(field(dialog, 'Worker lcore'));
      expect(group(dialog, g)).not.toContainElement(field(dialog, 'Ingress'));
    }
  });

  it('an unset worker reads "auto"', async () => {
    const { dialog } = await inspect('a-b');
    expect(field(dialog, 'Worker lcore').textContent?.trim()).toBe('auto');
  });

  it('a vdev port (no interface entry) stands alone with its device name', async () => {
    const { dialog } = await inspect('loop');
    expect(field(dialog, 'Ingress').textContent?.trim()).toBe('net_null0');
    expect(field(dialog, 'Egress').textContent?.trim()).toBe('net_null0');
  });

  it('a failing getInterfaces still opens the panel with device names and no alert', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    const { dialog } = await inspect('a-b');
    expect(field(dialog, 'Ingress').textContent?.trim()).toBe('0000:65:00.0');
    expect(field(dialog, 'Egress').textContent?.trim()).toBe('0000:65:00.1');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('has exactly the five subsections of the editor, in its order', async () => {
    const { dialog } = await inspect('a-b');
    expect(within(dialog).getAllByRole('group')).toHaveLength(5);
    const names = ['Buffer', 'Delayer', 'Limiter', 'Dropper', 'Statistics'];
    for (let i = 0; i + 1 < names.length; i += 1) {
      const x = group(dialog, names[i]);
      const y = group(dialog, names[i + 1]);
      expect(x.compareDocumentPosition(y) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    expect(within(dialog).getAllByRole('button', { name: 'Close' })).toHaveLength(1);
  });

  it('default-ish config: delayer in readable units, everything inactive shown as off', async () => {
    const { dialog } = await inspect('a-b');
    const buffer = group(dialog, 'Buffer');
    expect(dts(buffer)).toEqual(['Buffer capacity', 'Reordering', 'Seed']);
    expect(field(buffer, 'Buffer capacity').textContent?.trim()).toBe('1000');
    expect(field(buffer, 'Reordering').textContent?.trim()).toBe('off');
    expect(field(buffer, 'Seed').textContent?.trim()).toBe('random');

    const delayer = group(dialog, 'Delayer');
    expect(dts(delayer)).toEqual(['Delay distribution', 'Mean delay', 'Delay std', 'Correlation', 'Initialization rate']);
    expect(field(delayer, 'Delay distribution').textContent?.trim()).toBe('Normal'); // spec19: the short name, like the list
    expect(field(delayer, 'Mean delay').textContent?.trim()).toBe('1 \u00b5s'); // 1000 ns
    expect(field(delayer, 'Delay std').textContent?.trim()).toBe('10 ns');
    expect(field(delayer, 'Correlation').textContent?.trim()).toBe('off');
    expect(field(delayer, 'Initialization rate').textContent?.trim()).toBe('12.5 Mpps');

    const limiter = group(dialog, 'Limiter');
    expect(dts(limiter)).toEqual(['Limiter']);
    expect(field(limiter, 'Limiter').textContent?.trim()).toBe('off');

    const dropper = group(dialog, 'Dropper');
    expect(dts(dropper)).toEqual(['Loss model', 'Random loss probability']);
    expect(field(dropper, 'Loss model').textContent?.trim()).toBe('Random');
    expect(field(dropper, 'Random loss probability').textContent?.trim()).toBe('0');

    const stats = group(dialog, 'Statistics');
    expect(dts(stats)).toEqual(['Delay statistics', 'Histogram bins', 'Histogram lo', 'Histogram hi']);
    expect(field(stats, 'Delay statistics').textContent?.trim()).toBe('on');
    for (const l of ['Histogram bins', 'Histogram lo', 'Histogram hi']) expect(field(stats, l).textContent?.trim()).toBe('automatic');
  });

  it('everything on: correlation with half-life, limiter with mode, rate and limit, Gilbert-Elliot, seed, reordering', async () => {
    const { dialog } = await inspect('b-a');
    const buffer = group(dialog, 'Buffer');
    expect(field(buffer, 'Buffer capacity').textContent?.trim()).toBe('5000');
    expect(field(buffer, 'Reordering').textContent?.trim()).toBe('on');
    expect(field(buffer, 'Seed').textContent?.trim()).toBe('42');

    const delayer = group(dialog, 'Delayer');
    expect(dts(delayer)).toEqual([
      'Delay distribution', 'Mean delay', 'Delay std', 'Correlation', 'Half-life period', 'Initialization rate',
    ]);
    expect(field(delayer, 'Delay distribution').textContent?.trim()).toBe('Gamma');
    expect(field(delayer, 'Mean delay').textContent?.trim()).toBe('2.5 \u00b5s');
    expect(field(delayer, 'Delay std').textContent?.trim()).toBe('300 ns');
    expect(field(delayer, 'Correlation').textContent?.trim()).toBe('on');
    expect(field(delayer, 'Half-life period').textContent?.trim()).toBe('4 ms');
    expect(field(delayer, 'Initialization rate').textContent?.trim()).toBe('8 Mpps');

    const limiter = group(dialog, 'Limiter');
    expect(dts(limiter)).toEqual(['Limiter', 'Limiter mode', 'Rate']); // spec20: no burst limit
    expect(field(limiter, 'Limiter').textContent?.trim()).toBe('on');
    expect(field(limiter, 'Limiter mode').textContent?.trim()).toBe('TokenBucketFilter');
    expect(field(limiter, 'Rate').textContent?.trim()).toBe('40 Gbit/s');

    const dropper = group(dialog, 'Dropper');
    expect(dts(dropper)).toEqual([
      'Loss model', 'Gilbert-Elliot p', 'Gilbert-Elliot r', 'Gilbert-Elliot k', 'Gilbert-Elliot h', 'Gilbert-Elliot starts in good state',
    ]);
    expect(field(dropper, 'Loss model').textContent?.trim()).toBe('GilbertElliot');
    expect(field(dropper, 'Gilbert-Elliot p').textContent?.trim()).toBe('0.01');
    expect(field(dropper, 'Gilbert-Elliot r').textContent?.trim()).toBe('0.2');
    expect(field(dropper, 'Gilbert-Elliot k').textContent?.trim()).toBe('0.9');
    expect(field(dropper, 'Gilbert-Elliot h').textContent?.trim()).toBe('0.3');
    expect(field(dropper, 'Gilbert-Elliot starts in good state').textContent?.trim()).toBe('no');

    // delay statistics off: only the switch is listed (the stored histogram is inactive)
    const stats = group(dialog, 'Statistics');
    expect(dts(stats)).toEqual(['Delay statistics']);
    expect(field(stats, 'Delay statistics').textContent?.trim()).toBe('off');
  });

  it('a configured histogram is shown in readable units (config3: 32 bins, 1 us .. 50 us), the loss probability without float noise', async () => {
    const link = fx.link({ config: fx.config3(), worker: null });
    const { dialog } = await inspect('a-b', [link]);
    const stats = group(dialog, 'Statistics');
    expect(field(stats, 'Delay statistics').textContent?.trim()).toBe('on');
    expect(field(stats, 'Histogram bins').textContent?.trim()).toBe('32');
    expect(field(stats, 'Histogram lo').textContent?.trim()).toBe('1 \u00b5s');
    expect(field(stats, 'Histogram hi').textContent?.trim()).toBe('50 \u00b5s');
    expect(field(group(dialog, 'Dropper'), 'Random loss probability').textContent?.trim()).toBe('0.25');
    const lim = group(dialog, 'Limiter');
    expect(field(lim, 'Limiter mode').textContent?.trim()).toBe('Spacer');
    expect(field(lim, 'Rate').textContent?.trim()).toBe('5 Gbit/s');
    // correlation is off in config3: its half-life (555 ns) is not listed
    expect(dts(group(dialog, 'Delayer'))).not.toContain('Half-life period');
    expect(field(group(dialog, 'Delayer'), 'Mean delay').textContent?.trim()).toBe('7.777 \u00b5s');
  });

  it.each([
    [0.07, '0.07'], [0.29, '0.29'], [0.57, '0.57'], [0.001, '0.001'], [1, '1'],
  ])('loss probability %s reads "%s" (no float noise)', async (p, text) => {
    const link = fx.link({ config: fx.config({ loss_model: { type: 'Random', p } }) });
    const { dialog } = await inspect('a-b', [link]);
    expect(field(group(dialog, 'Dropper'), 'Random loss probability').textContent?.trim()).toBe(text);
  });

  it('the panel shows the link that was clicked (two links, two different panels)', async () => {
    const { user, dialog } = await inspect('a-b');
    expect(field(group(dialog, 'Delayer'), 'Mean delay').textContent?.trim()).toBe('1 \u00b5s');
    await user.click(within(dialog).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: 'Inspect b-a' }));
    const second = await screen.findByRole('dialog', { name: 'b-a' });
    expect(field(group(second, 'Delayer'), 'Mean delay').textContent?.trim()).toBe('2.5 \u00b5s');
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
  });

  it('the Edit action in the panel is a link to the editor of that link', async () => {
    const { user, dialog } = await inspect('b-a');
    const edit = within(dialog).getByRole('link', { name: 'Edit b-a' });
    expect(edit).toHaveAttribute('href', '/links/3');
    await user.click(edit);
    expect(await screen.findByRole('heading', { level: 1, name: 'Edit link' })).toBeInTheDocument();
    expect(api.getLink).toHaveBeenCalledWith(3);
  });

  it('inspecting changes nothing: no save, no delete, no extra interface request', async () => {
    await inspect('a-b');
    expect(api.updateLink).not.toHaveBeenCalled();
    expect(api.createLink).not.toHaveBeenCalled();
    expect(api.deleteLink).not.toHaveBeenCalled();
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
  });

  it('the delete flow still works next to the panel (one dialog at a time, confirm deletes)', async () => {
    const { user, dialog } = await inspect('a-b');
    await user.click(within(dialog).getByRole('button', { name: 'Close' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await user.click(screen.getByRole('button', { name: 'Delete a-b' }));
    const confirm = await screen.findByRole('dialog');
    expect(confirm).toHaveTextContent('a-b');
    await user.click(within(confirm).getByRole('button', { name: 'Confirm delete' }));
    expect(api.deleteLink).toHaveBeenCalledWith(1);
  });
});

// v16 per-section columns (cell index 2..5; each cell is one element whose own text is exactly the part):
//   Delay       "<dist> <mean> / <std>" with dist Normal | LogNormal | Gamma | Uniform (the enum name without
//               "Distribution"); Deterministic shows the mean only ("Deterministic 10 ms"), also when a std is stored.
//               mean and std each use the editor's display-unit rule on their own ("10 ms", "2.5 µs", "0 ms").
//   Reordering  "+" when on, "\u2212" (U+2212, not a hyphen) when off.
//   Limiter     "spacing <rate>" (Spacer), "policing <rate>" (TokenBucketFilter), "none" when inactive; the rate in its
//               display unit ("10 Gbit/s", "1.5 kbit/s"); the burst limit is not shown.
//   Loss        "Random p=<p>" or "Gilbert-Elliott p=<p>, r=<r>, h=<h>, k=<k>" (this order, ", " between), numbers as
//               stored (no float noise). No literal brackets, parentheses or PCI addresses; no " · " separators.
describe('Links list: Delay, Reordering, Limiter and Loss columns (v16)', () => {
  const MU = '\u00b5s';
  const MINUS = '\u2212';
  const DELAY = 2;
  const REORDER = 3;
  const LIMITER = 4;
  const LOSS = 5;
  const cellOf = async (name: string, idx: number) => within(await rowOf(name)).getAllByRole('cell')[idx].textContent ?? '';
  const withConfig = (over: Parameters<typeof fx.config>[0] = {}, d: object = {}, l: object = {}) => {
    const base = fx.config();
    return fx.link({
      config: { ...base, ...over, delayer: { ...base.delayer, ...d }, limiter: { ...base.limiter, ...l } },
    });
  };

  it('the three fixture configs read exactly (default-ish, everything on with Gilbert-Elliot, LogNormal with Spacer)', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev(), fx.link({ id: 4, name: 'c3', config: fx.config3() })]);
    renderApp('/links');
    await screen.findByText('a-b');
    const cells = async (n: string) => [await cellOf(n, DELAY), await cellOf(n, REORDER), await cellOf(n, LIMITER), await cellOf(n, LOSS)];
    expect(await cells('a-b')).toEqual([`Normal 1 ${MU} / 10 ns`, MINUS, 'none', 'Random p=0']);
    expect(await cells('b-a')).toEqual([
      `Gamma 2.5 ${MU} / 300 ns`, '+', 'policing 40 Gbit/s', 'Gilbert-Elliott p=0.01, r=0.2, h=0.3, k=0.9',
    ]);
    expect(await cells('c3')).toEqual([`LogNormal 7.777 ${MU} / 777 ns`, '+', 'spacing 5 Gbit/s', 'Random p=0.25']);
  });

  it('each cell is one element whose own text is exactly the part, in the row of its link', async () => {
    renderApp('/links');
    const row = await rowOf('a-b');
    for (const text of [`Normal 1 ${MU} / 10 ns`, MINUS, 'none', 'Random p=0']) {
      const el = within(row).getByText(text);
      expect(el.textContent).toBe(text);
      expect(el.closest('tr')).toBe(row);
    }
  });

  it.each([
    ['DeterministicDistribution', 'Deterministic 10 ms'],
    ['NormalDistribution', 'Normal 10 ms / 1 ms'],
    ['LogNormalDistribution', 'LogNormal 10 ms / 1 ms'],
    ['GammaDistribution', 'Gamma 10 ms / 1 ms'],
    ['UniformDistribution', 'Uniform 10 ms / 1 ms'],
  ] as const)('%s reads "%s" (the deterministic one shows the mean only, even with a std stored)', async (distribution, text) => {
    api.getLinks.mockResolvedValue([withConfig({}, { distribution, mean_ns: 10000000, std_ns: 1000000 })]);
    renderApp('/links');
    expect(await cellOf('a-b', DELAY)).toBe(text);
  });

  it.each([
    [1000, 10, `Normal 1 ${MU} / 10 ns`],
    [2500, 1000000, `Normal 2.5 ${MU} / 1 ms`],
    [0, 0, 'Normal 0 ms / 0 ms'],
    [1500000000, 1234567, `Normal 1.5 s / 1234.567 ${MU}`],
    [999, 1001, `Normal 999 ns / 1.001 ${MU}`],
  ])('mean %d ns and std %d ns read "%s" (each in its own unit)', async (mean_ns, std_ns, text) => {
    api.getLinks.mockResolvedValue([withConfig({}, { mean_ns, std_ns })]);
    renderApp('/links');
    expect(await cellOf('a-b', DELAY)).toBe(text);
  });

  it.each([[true, '+'], [false, MINUS]])('reordering %s reads "%s"', async (reordering, text) => {
    api.getLinks.mockResolvedValue([withConfig({ reordering })]);
    renderApp('/links');
    expect(await cellOf('a-b', REORDER)).toBe(text);
  });

  it('the reordering minus is U+2212, not a hyphen', async () => {
    renderApp('/links');
    expect((await cellOf('a-b', REORDER)).charCodeAt(0)).toBe(0x2212);
  });

  it.each([
    ['Spacer', true, 10000000000, 'spacing 10 Gbit/s'],
    ['TokenBucketFilter', true, 10000000000, 'policing 10 Gbit/s'],
    ['Spacer', true, 1500, 'spacing 1.5 kbit/s'],
    ['TokenBucketFilter', true, 2500000, 'policing 2.5 Mbit/s'],
    ['Spacer', true, 999, 'spacing 999 bit/s'],
    ['Spacer', false, 10000000000, 'none'],
    ['TokenBucketFilter', false, 40000000000, 'none'],
  ] as const)('limiter %s active=%s rate %d reads "%s"', async (mode, activate, rate_bps, text) => {
    api.getLinks.mockResolvedValue([withConfig({}, {}, { mode, activate, rate_bps })]);
    renderApp('/links');
    expect(await cellOf('a-b', LIMITER)).toBe(text);
  });

  it.each([
    [0.07, 'Random p=0.07'], [0.29, 'Random p=0.29'], [0.57, 'Random p=0.57'], [0.001, 'Random p=0.001'], [0, 'Random p=0'], [1, 'Random p=1'],
  ])('Random loss %s reads "%s" (no float noise)', async (p, text) => {
    api.getLinks.mockResolvedValue([withConfig({ loss_model: { type: 'Random', p } })]);
    renderApp('/links');
    expect(await cellOf('a-b', LOSS)).toBe(text);
  });

  it('Gilbert-Elliott loss lists p, r, h, k in this order, whatever the stored key order', async () => {
    api.getLinks.mockResolvedValue([
      withConfig({ loss_model: { type: 'GilbertElliot', k: 0.5, h: 0.25, r: 0.125, p: 0.0625, state_is_good: false } }),
    ]);
    renderApp('/links');
    expect(await cellOf('a-b', LOSS)).toBe('Gilbert-Elliott p=0.0625, r=0.125, h=0.25, k=0.5');
  });

  it('the cells carry no separators, brackets, parentheses, PCI addresses or worker text', async () => {
    api.getLinks.mockResolvedValue([fx.linkRev({ worker: 3 })]);
    renderApp('/links');
    for (const idx of [DELAY, REORDER, LIMITER, LOSS]) {
      const text = await cellOf('b-a', idx);
      expect(text).not.toMatch(/[()[\]<>{}\u00b7]/);
      expect(text).not.toMatch(/0000:|auto|worker|lcore|reordering/i);
    }
  });

  it('each row has its own cells', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev(), fx.loopLink()]);
    renderApp('/links');
    const a = [await cellOf('a-b', DELAY), await cellOf('a-b', LOSS)];
    const b = [await cellOf('b-a', DELAY), await cellOf('b-a', LOSS)];
    const c = [await cellOf('loop', DELAY), await cellOf('loop', LOSS)];
    expect(a).not.toEqual(b);
    expect(c).toEqual(a); // loopLink uses config() like a-b
  });

  it('the cells do not depend on the interface list (they show with a failing getInterfaces)', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/links');
    expect(await cellOf('a-b', DELAY)).toBe(`Normal 1 ${MU} / 10 ns`);
    expect(await cellOf('a-b', LOSS)).toBe('Random p=0');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('the inspect panel is still the full view next to the compact columns', async () => {
    const user = userEvent.setup();
    renderApp('/links');
    await screen.findByText('a-b');
    await user.click(screen.getByRole('button', { name: 'Inspect a-b' }));
    const dialog = await screen.findByRole('dialog', { name: 'a-b' });
    expect(within(dialog).getAllByRole('group')).toHaveLength(5);
  });
});
