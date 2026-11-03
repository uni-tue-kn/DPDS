import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { deferred, renderApp } from '../test/helpers';

// v12 labels, see the v12 block of src/test/helpers.tsx.
// Ports read "<pci> (<netdev>)" (netdevs of fx.interfaces()); a port without netdev or interface shows only its name.
const AB = 'a-b: 0000:65:00.0 (enp101s0f0) → 0000:65:00.1 (enp101s0f1)';
const BA = 'b-a: 0000:65:00.1 (enp101s0f1) → 0000:65:00.0 (enp101s0f0)';
const LOOP = 'loop: net_null0 → net_null0';

describe('Links list', () => {
  it('lists one row per link reading "name: ingress → egress"', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev(), fx.loopLink()]);
    renderApp('/links');
    const row = (await screen.findByText(AB)).closest('tr') as HTMLElement;
    expect(row).toBeInTheDocument();
    const rev = screen.getByText(BA).closest('tr') as HTMLElement;
    expect(rev).not.toBe(row);
    const loop = screen.getByText(LOOP).closest('tr') as HTMLElement;
    expect(loop).not.toHaveTextContent('0000:65:00');
    expect(loop).not.toHaveTextContent('(');
    expect(screen.getAllByRole('row')).toHaveLength(1 + 3); // header + one row per link, not per direction
  });

  it('the old "a ↔ b" pair notation and the direction count are gone', async () => {
    api.getLinks.mockResolvedValue([fx.link()]);
    renderApp('/links');
    const row = (await screen.findByText(AB)).closest('tr') as HTMLElement;
    expect(row).not.toHaveTextContent('↔');
    expect(screen.queryByRole('columnheader', { name: /directions/i })).not.toBeInTheDocument();
  });

  it('shows the worker of a link, "auto" without one', async () => {
    api.getLinks.mockResolvedValue([fx.link({ worker: 7 }), fx.linkRev({ worker: null })]);
    renderApp('/links');
    const row = (await screen.findByText(AB)).closest('tr') as HTMLElement;
    expect(within(row).getByText('7')).toBeInTheDocument();
    expect(row).not.toHaveTextContent(/auto/i);
    const rev = screen.getByText(BA).closest('tr') as HTMLElement;
    expect(rev).toHaveTextContent(/auto/i);
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

  it('shows the pending-restart notice only when the emulation says so', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running', { pending_restart: true }));
    const view = renderApp('/links');
    expect(await screen.findByTestId('pending-restart')).toHaveTextContent(/restart/i);
    view.unmount();
    api.getEmulation.mockClear();
    api.getEmulation.mockResolvedValue(fx.emulation('running', { pending_restart: false }));
    renderApp('/links');
    await screen.findByText(AB);
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
    await waitFor(() => expect(screen.queryByText(AB)).not.toBeInTheDocument());
    expect(screen.getByText('loop: net_null0 → net_null0')).toBeInTheDocument();
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

describe('Links list: port labels with interface names (v12)', () => {
  it('shows PCI address and netdev of both ports', async () => {
    renderApp('/links');
    const cell = await screen.findByText(AB);
    expect(cell.closest('tr')).toHaveTextContent(AB);
  });

  it('the label is one element whose own text is the whole "name: ingress (netdev) → egress (netdev)"', async () => {
    renderApp('/links');
    const el = await screen.findByText(AB);
    expect(el.textContent).toBe(AB);
  });

  it('a hairpin on a real NIC shows the same label twice', async () => {
    api.getLinks.mockResolvedValue([fx.link({ id: 5, name: 'hair', ingress: '0000:65:00.1', egress: '0000:65:00.1' })]);
    renderApp('/links');
    expect(await screen.findByText('hair: 0000:65:00.1 (enp101s0f1) → 0000:65:00.1 (enp101s0f1)')).toBeInTheDocument();
  });

  it('a port whose interface has no netdev (vfio-bound) shows only the PCI address', async () => {
    api.getInterfaces.mockResolvedValue([
      fx.iface({ pci: '0000:65:00.0', netdev: null }),
      fx.iface({ pci: '0000:65:00.1', netdev: 'enp101s0f1' }),
    ]);
    renderApp('/links');
    expect(await screen.findByText('a-b: 0000:65:00.0 → 0000:65:00.1 (enp101s0f1)')).toBeInTheDocument();
  });

  it('a port that is not in the interface list shows only its name', async () => {
    api.getInterfaces.mockResolvedValue([fx.iface({ pci: '0000:65:00.0', netdev: 'enp101s0f0' })]);
    renderApp('/links');
    expect(await screen.findByText('a-b: 0000:65:00.0 (enp101s0f0) → 0000:65:00.1')).toBeInTheDocument();
  });

  it('a vdev next to a NIC: only the NIC gets a name', async () => {
    api.getLinks.mockResolvedValue([fx.link({ id: 6, name: 'mix', ingress: 'net_null0', egress: '0000:65:00.0' })]);
    renderApp('/links');
    expect(await screen.findByText('mix: net_null0 → 0000:65:00.0 (enp101s0f0)')).toBeInTheDocument();
  });

  it('an empty interface list leaves device names only', async () => {
    api.getInterfaces.mockResolvedValue([]);
    renderApp('/links');
    expect(await screen.findByText('a-b: 0000:65:00.0 → 0000:65:00.1')).toBeInTheDocument();
  });

  it('a failing getInterfaces falls back to device names without any alert', async () => {
    api.getInterfaces.mockRejectedValue(new ApiError(500, 'sysfs unreadable'));
    renderApp('/links');
    expect(await screen.findByText('a-b: 0000:65:00.0 → 0000:65:00.1')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/sysfs unreadable/);
    // the rest of the page works: edit link and delete button of the row are there
    expect(screen.getByRole('link', { name: 'Edit a-b' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete a-b' })).toBeInTheDocument();
  });

  it('a failing getLinks still shows its own error (the interfaces fallback does not swallow it)', async () => {
    api.getLinks.mockRejectedValue(new ApiError(500, 'state file is corrupt'));
    renderApp('/links');
    expect(await screen.findByRole('alert')).toHaveTextContent('state file is corrupt');
  });

  it('a slow interface list does not delay the rows: they show plain names first, then the labels', async () => {
    const d = deferred<ReturnType<typeof fx.interfaces>>();
    api.getInterfaces.mockReturnValue(d.promise);
    renderApp('/links');
    expect(await screen.findByText('a-b: 0000:65:00.0 → 0000:65:00.1')).toBeInTheDocument();
    d.resolve(fx.interfaces());
    expect(await screen.findByText(AB)).toBeInTheDocument();
    expect(screen.queryByText('a-b: 0000:65:00.0 → 0000:65:00.1')).not.toBeInTheDocument();
  });

  it('asks for the interfaces once per page load, also after a delete reloads the list', async () => {
    const user = userEvent.setup();
    api.getLinks.mockResolvedValueOnce([fx.link(), fx.loopLink()]).mockResolvedValue([fx.loopLink()]);
    renderApp('/links');
    await screen.findByText(AB);
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Delete a-b' }));
    await user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm delete' }));
    await waitFor(() => expect(screen.queryByText(AB)).not.toBeInTheDocument());
    expect(api.getInterfaces).toHaveBeenCalledTimes(1);
  });
});

describe('Links list: loss text (v12, review low)', () => {
  const lossLink = (p: number) => fx.link({ config: fx.config({ loss_model: { type: 'Random', p } }) });

  // formatPct (common/format.ts) prints `${v.toFixed(2)} %`, applied to p * 100.
  it.each([
    [0.07, '7.00 % loss'], // 0.07 * 100 = 7.000000000000001
    [0.29, '29.00 % loss'], // 0.29 * 100 = 28.999999999999996
    [0.57, '57.00 % loss'], // 0.57 * 100 = 56.99999999999999
    [0.5, '50.00 % loss'],
    [0.001, '0.10 % loss'],
    [0, '0.00 % loss'],
    [1, '100.00 % loss'],
  ])('p %s reads "%s"', async (p, text) => {
    api.getLinks.mockResolvedValue([lossLink(p)]);
    renderApp('/links');
    const row = (await screen.findByRole('link', { name: 'Edit a-b' })).closest('tr') as HTMLElement;
    expect(row).toHaveTextContent(text);
    expect(row.textContent).not.toMatch(/\d\.\d{3,}/); // no float noise anywhere in the row
  });

  it('a Gilbert-Elliot loss model still reads "burst loss"', async () => {
    api.getLinks.mockResolvedValue([fx.linkRev()]);
    renderApp('/links');
    const row = (await screen.findByRole('link', { name: 'Edit b-a' })).closest('tr') as HTMLElement;
    expect(row).toHaveTextContent('burst loss');
  });
});
