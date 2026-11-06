// User flows of the Host hints page: the list, the hints for the local links, Recheck and the commands buttons.
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Hint } from '../api/types';
import * as fx from '../test/fixtures';
import { deferred, expectStartBody, renderApp, seedStore } from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';

const group = (name: RegExp) => screen.getByRole('region', { name });
const itemOf = (title: string) => must(screen.getByText(title).closest('li'), `the list item of "${title}"`);

/** user-event installs its own clipboard stub in `setup()`, so a test's clipboard goes in afterwards. */
function setClipboard(writeText: ((text: string) => Promise<void>) | undefined) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: writeText && { writeText } });
}

describe('Host hints: the list', () => {
  it('puts every hint into the group of its level with title, detail and suggestion, and omits empty groups', async () => {
    api.getHostHints.mockResolvedValue(fx.hints().filter((h) => h.level !== 'info'));
    renderApp('/hints');
    const errors = await screen.findByRole('region', { name: /^Errors/ });
    expect(within(errors).getByText('No core isolation')).toBeInTheDocument();
    expect(within(errors).getByText('isolcpus is not set.')).toBeInTheDocument();
    expect(within(errors).getByText(/isolcpus=2-5/)).toBeInTheDocument();
    const warnings = group(/^Warnings/);
    expect(within(warnings).getByText('Few hugepages')).toBeInTheDocument();
    expect(within(warnings).getByText('Governor is powersave')).toBeInTheDocument();
    expect(within(group(/^OK/)).getByText('NIC IRQs are off the isolated cores')).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: /^Info/ })).not.toBeInTheDocument();
  });

  it('says so when there are no hints', async () => {
    api.getHostHints.mockResolvedValue([]);
    renderApp('/hints');
    expect(await screen.findByText('No hints.')).toBeInTheDocument();
  });

  it('shows the API error', async () => {
    api.getHostHints.mockRejectedValue(new Error('controller exploded'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('controller exploded');
  });
});

describe('Host hints: hints for the local links', () => {
  it('without stored links only the host hints are asked for', async () => {
    renderApp('/hints');
    await screen.findByText('No core isolation');
    expect(api.getHostHintsFor).not.toHaveBeenCalled();
  });

  it('with stored links the start body is POSTed and the answers are merged by id, the POST answer winning', async () => {
    const doc = seedStore({ links: [fx.link(), fx.linkRev()] });
    api.getHostHintsFor.mockResolvedValue([
      { id: 'hugepages', level: 'error', title: 'No hugepages for these links', detail: 'd', suggestion: 's' },
      { id: 'link_only', level: 'info', title: 'Only for the links', detail: 'd', suggestion: 's' },
    ]);
    renderApp('/hints');
    expect(await screen.findByText('Only for the links')).toBeInTheDocument();
    expectStartBody(api.getHostHintsFor.mock.calls[0]?.[0], doc);
    expect(within(group(/^Errors/)).getByText('No hugepages for these links')).toBeInTheDocument();
    expect(screen.queryByText('Few hugepages')).not.toBeInTheDocument();
    expect(screen.getByText('No core isolation')).toBeInTheDocument();
  });

  it('a failing POST shows its message while the host hints stay', async () => {
    seedStore({ links: [fx.link()] });
    api.getHostHintsFor.mockRejectedValue(new Error('cannot check these links'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('cannot check these links');
    expect(screen.getByText('No core isolation')).toBeInTheDocument();
  });
});

describe('Host hints: Recheck', () => {
  it('asks the controller again with recheck for the host hints and the link hints, once per click, and shows the new answer', async () => {
    const user = userEvent.setup();
    const doc = seedStore({ links: [fx.link()] });
    const hostAgain = deferred<Hint[]>();
    api.getHostHints.mockResolvedValueOnce(fx.hints()).mockImplementationOnce(() => hostAgain.promise);
    renderApp('/hints');
    await screen.findByText('No core isolation');
    expect(api.getHostHints).toHaveBeenLastCalledWith(undefined);
    expect(api.getHostHintsFor.mock.calls[0]?.[1]).toBeUndefined();

    await user.dblClick(screen.getByRole('button', { name: 'Recheck' }));
    expect(api.getHostHints).toHaveBeenCalledTimes(2);
    expect(api.getHostHints).toHaveBeenLastCalledWith({ recheck: true });
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(2);
    expectStartBody(api.getHostHintsFor.mock.calls[1]?.[0], doc);
    expect(api.getHostHintsFor.mock.calls[1]?.[1]).toEqual({ recheck: true });

    // while it runs the old hints stay and the button is off
    expect(screen.getByRole('button', { name: 'Recheck' })).toBeDisabled();
    expect(screen.getByText('No core isolation')).toBeInTheDocument();

    hostAgain.resolve([{ id: 'fresh', level: 'ok', title: 'Everything is fine now', detail: 'd', suggestion: 's' }]);
    expect(await screen.findByText('Everything is fine now')).toBeInTheDocument();
    expect(screen.queryByText('No core isolation')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Recheck' })).toBeEnabled());
  });

  it('a failed recheck shows its message and the button works again', async () => {
    const user = userEvent.setup();
    api.getHostHints.mockResolvedValueOnce(fx.hints()).mockRejectedValueOnce(new Error('recheck failed'));
    renderApp('/hints');
    await screen.findByText('No core isolation');
    await user.click(screen.getByRole('button', { name: 'Recheck' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('recheck failed');
    await waitFor(() => expect(screen.getByRole('button', { name: 'Recheck' })).toBeEnabled());
  });
});

describe('Host hints: commands', () => {
  const swap = fx.commandsHint({ commands: ['sysctl -w vm.swappiness=0', 'echo "a > b" > /tmp/x'] });

  async function showCommands() {
    api.getHostHints.mockResolvedValue([swap, ...fx.hints()]);
    renderApp('/hints');
    await screen.findByText(swap.title);
  }

  it('only a hint with commands has the buttons; hovering the icon button lists them', async () => {
    const user = userEvent.setup();
    await showCommands();
    expect(screen.getAllByRole('button', { name: 'Copy commands' })).toHaveLength(1);
    await user.hover(within(itemOf(swap.title)).getByRole('button', { name: `Commands for ${swap.title}` }));
    const tip = await screen.findByRole('tooltip');
    for (const c of must(swap.commands)) expect(tip).toHaveTextContent(c);
  });

  it('Copy commands writes the commands joined by newlines to the clipboard and confirms', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    await showCommands();
    setClipboard(writeText);
    await user.click(within(itemOf(swap.title)).getByRole('button', { name: 'Copy commands' }));
    expect(writeText).toHaveBeenCalledExactlyOnceWith(must(swap.commands).join('\n'));
    expect(await within(itemOf(swap.title)).findByText('Copied')).toBeInTheDocument();
  });

  it.each([
    ['no clipboard (plain http)', undefined],
    ['a rejected write', () => Promise.reject(new DOMException('denied', 'NotAllowedError'))],
  ])('with %s the commands are shown for copying by hand', async (_name, writeText) => {
    const user = userEvent.setup();
    await showCommands();
    setClipboard(writeText);
    await user.click(within(itemOf(swap.title)).getByRole('button', { name: 'Copy commands' }));
    const pre = await waitFor(() => must(itemOf(swap.title).querySelector('pre')));
    expect(pre.textContent).toBe(must(swap.commands).join('\n'));
    expect(within(itemOf(swap.title)).queryByText('Copied')).not.toBeInTheDocument();
  });
});
