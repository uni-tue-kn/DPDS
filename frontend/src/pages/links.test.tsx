import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp } from '../test/helpers';

describe('Links list', () => {
  const AB = 'a-b: 0000:65:00.0 → 0000:65:00.1';
  const LOOP = 'loop: net_null0 → net_null0';

  it('lists one row per link reading "name: ingress → egress"', async () => {
    api.getLinks.mockResolvedValue([fx.link(), fx.linkRev(), fx.loopLink()]);
    renderApp('/links');
    const row = (await screen.findByText(AB)).closest('tr') as HTMLElement;
    expect(row).toBeInTheDocument();
    const rev = screen.getByText('b-a: 0000:65:00.1 → 0000:65:00.0').closest('tr') as HTMLElement;
    expect(rev).not.toBe(row);
    const loop = screen.getByText(LOOP).closest('tr') as HTMLElement;
    expect(loop).not.toHaveTextContent('0000:65:00');
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
    const rev = screen.getByText('b-a: 0000:65:00.1 → 0000:65:00.0').closest('tr') as HTMLElement;
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
    await screen.findByText('a-b: 0000:65:00.0 → 0000:65:00.1');
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
    await waitFor(() => expect(screen.queryByText('a-b: 0000:65:00.0 → 0000:65:00.1')).not.toBeInTheDocument());
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
