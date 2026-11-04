import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi } from 'vitest';
import type { Hint, HintLevel } from '../api/types';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { renderApp, seedStore } from '../test/helpers';
import { must } from '../test/must';

// Spec 33 (frontend, items 9-10). User decision 2026-11-04: DPDS never applies host tuning; hints only SUGGEST the commands
// an administrator runs as root. Contract pinned here:
// - A hint with a non-empty `commands: string[]` shows, inside its own listitem, a button named exactly
//   `Commands for <hint title>` (terminal icon) and a button named exactly `Copy commands`, both visible without any
//   interaction. A hint without `commands` (absent or empty array) has neither, nor any other button.
// - Hovering or keyboard-focusing the Commands button shows ONE role=tooltip with every command of THAT hint, each inside
//   a monospace block (`code` or `pre`), and the note "Run as root on the DPDS host."; moving away or blurring hides it.
//   CHOICE: the note is part of the tooltip (the brief says so), not pinned to be visible without the tooltip.
// - "Copy commands" calls `navigator.clipboard.writeText` ONCE with the commands joined by "\n" (no trailing newline) and
//   sends no request and writes no local document. CHOICE: the button is in the listitem, not in the tooltip (a tooltip
//   closes when the pointer leaves it, a button inside could not be reached by keyboard).
// - No Apply button, no dialog, no `applyHostAction`, no `/host/actions` anywhere in the frontend source, and no
//   apply/undo/revert/reset control. The page does not read the emulation state any more (`api.getEmulation` is not called).
// Spec 33b (frontend item 1), review of 378b16a: commands must be readable and copyable on the plain-http deployment.
// - The tooltip's root element (role=tooltip) carries the class `commands-tip`; the stylesheet gives it a wide max-width and
//   a wrapping `pre` (pinned in `assets/stylesSpec33b.test.ts`, since jsdom renders no CSS).
// - When `navigator.clipboard` is missing, `writeText` rejects or `writeText` throws, "Copy commands" reveals, inside the
//   hint's own list item, ONE `pre` (outside any tooltip, not hidden, not user-select:none) whose text is exactly the
//   commands joined by "\n", plus the text FALLBACK_TEXT in an element of its own. No "Copied" then.
// - On success the list item gets a role=status element with the text "Copied" (not before the write resolved), which is
//   gone after about 2 s (present at 1 s, gone at 3 s, timer restarts on a second copy). No fallback then.
// CHOICES of the tester (spec silent): the feedback sits in the hint's list item; per-hint state (another hint shows
// nothing); a second click on Copy keeps the fallback revealed (no toggle); a synchronous throw of `writeText` counts as a
// failure; the fallback adds no button that the "applies nothing" rules would forbid.
// Not pinned: the icon class, the tooltip placement, how the fallback pre is styled, whether an empty status element stays.

const NOTE = 'Run as root on the DPDS host.';
const FALLBACK_TEXT = 'Clipboard not available here (plain http): select the commands and copy them.';
const NIC = '0000:65:00.0';
const SWAP = fx.commandsHint();
const FLOW = fx.commandsHint({
  id: `flow_control:${NIC}`,
  level: 'warn',
  title: 'Flow control is on for enp101s0f0',
  detail: 'rx on, tx on.',
  suggestion: 'Turn flow control off.',
  commands: ['ethtool -A enp101s0f0 rx off tx off'],
});
const FIRMWARE = fx.commandsHint({
  id: `firmware:${NIC}`,
  level: 'warn',
  title: 'Firmware PCIe read tuning is missing on enp101s0f0',
  detail:
    'MAX_ACC_OUT_READ is 0, target 256. Persists in NVRAM and needs a firmware reset; on a VM run it on the hypervisor host.',
  suggestion: 'Set the firmware values.',
  commands: [
    `mstconfig -d ${NIC} -y set ADVANCED_PCI_SETTINGS=1`,
    `mstconfig -d ${NIC} -y set MAX_ACC_OUT_READ=256 CQE_COMPRESSION=1`,
  ],
});
const IRQ = fx.commandsHint({
  id: 'irq_affinity_commands', // not the fixture's `irq_affinity`: hints are merged by id
  level: 'warn',
  title: 'NIC IRQs run on isolated cores',
  detail: 'IRQ 123 is on core 3.',
  suggestion: 'Move the IRQs off the isolated cores.',
  commands: ['echo 2-15 > /proc/irq/123/smp_affinity_list', 'echo 2-15 > /proc/irq/124/smp_affinity_list'],
});

const itemOf = (title: string) => must(screen.getByText(title).closest('li'), `the list item of "${title}"`);
const commandsButton = (title: string) => within(itemOf(title)).getByRole('button', { name: `Commands for ${title}` });
const copyButton = (title: string) => within(itemOf(title)).getByRole('button', { name: 'Copy commands' });
/** The text of the page proper (without the sidebar). */
const pageOf = () => must(screen.getByRole('heading', { level: 1 }).parentElement, 'the page');

/** Renders the page with these hints as the host-only answer and waits until they are listed. */
async function showHints(hints: Hint[]) {
  api.getHostHints.mockResolvedValue(hints);
  const view = renderApp('/hints');
  await screen.findByText(must(hints[0]).title);
  return view;
}

/**
 * user-event's `setup()` installs its own clipboard stub, so the mock goes in after it. The code under test must read
 * `navigator.clipboard` when the button is clicked, not when the module loads.
 */
function setClipboard(writeText: ((text: string) => Promise<void>) | undefined) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: writeText && { writeText } });
}

function mockClipboard(writeText = vi.fn().mockResolvedValue(undefined)) {
  setClipboard(writeText);
  return writeText;
}

/** Every command of `hint` is inside a `code` or `pre` element of `tip`. */
function expectMonospace(tip: HTMLElement, hint: Hint) {
  const blocks = Array.from(tip.querySelectorAll('code, pre')).map((b) => b.textContent ?? '');
  expect(blocks.length, 'a code or pre block in the tooltip').toBeGreaterThan(0);
  // Each command is a line of its own: the whole text of a code element, or one line of a pre (newline-joined).
  const lines = blocks.flatMap((b) => b.split('\n').map((l) => l.trim()));
  for (const c of must(hint.commands)) expect(lines, `"${c}" on a line of its own in a code/pre block`).toContain(c);
}

describe('Host hints: the commands buttons', () => {
  it('a hint with commands shows both buttons inside its own list item, with exact names', async () => {
    await showHints([SWAP, FLOW]);
    expect(commandsButton(SWAP.title)).toBeInTheDocument();
    expect(copyButton(SWAP.title)).toBeInTheDocument();
    expect(commandsButton(FLOW.title)).toBeInTheDocument();
    expect(copyButton(FLOW.title)).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /^Commands for / })).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: 'Copy commands' })).toHaveLength(2);
    // the name is of THIS hint, not another one's
    expect(within(itemOf(SWAP.title)).queryByRole('button', { name: `Commands for ${FLOW.title}` })).toBeNull();
  });

  it('the buttons are plain buttons that do not submit anything and are enabled', async () => {
    await showHints([SWAP]);
    for (const b of [commandsButton(SWAP.title), copyButton(SWAP.title)]) {
      expect(b).toBeEnabled();
      expect(b).toHaveAttribute('type', 'button');
    }
  });

  it('the Commands button is an icon button: its accessible name is the aria-label, not visible text', async () => {
    await showHints([SWAP]);
    const b = commandsButton(SWAP.title);
    expect(b).toHaveAttribute('aria-label', `Commands for ${SWAP.title}`);
    expect(b.querySelector('i, svg')).not.toBeNull(); // the terminal icon
    expect(b.textContent?.trim()).toBe('');
  });

  it('a hint without commands has no button at all, also next to hints that have them', async () => {
    await showHints([SWAP, ...fx.hints()]);
    for (const h of fx.hints()) expect(within(itemOf(h.title)).queryAllByRole('button')).toEqual([]);
    expect(screen.getAllByRole('button', { name: /^Commands for / })).toHaveLength(1);
    expect(screen.getAllByRole('button', { name: 'Copy commands' })).toHaveLength(1);
  });

  it('an empty commands array counts as no commands: no buttons', async () => {
    await showHints([fx.commandsHint({ commands: [] }), ...fx.hints()]);
    expect(within(itemOf(SWAP.title)).queryAllByRole('button')).toEqual([]);
    expect(screen.queryAllByRole('button', { name: /commands/i })).toEqual([]);
  });

  it('with the default fixture hints (none has commands) there is no commands button on the page', async () => {
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    expect(screen.queryAllByRole('button', { name: /commands/i })).toEqual([]);
    expect(within(pageOf()).queryAllByRole('button')).toEqual([]);
  });

  it.each<HintLevel>(['error', 'warn', 'info', 'ok'])('a %s hint with commands gets the buttons too', async (level) => {
    const hint = fx.commandsHint({ level, title: `Hint of level ${level}` });
    await showHints([hint]);
    expect(commandsButton(hint.title)).toBeInTheDocument();
    expect(copyButton(hint.title)).toBeInTheDocument();
  });

  it('a hint from the link-dependent POST answer gets its buttons too', async () => {
    seedStore({ links: [fx.link()] });
    api.getHostHintsFor.mockResolvedValue([FLOW]);
    renderApp('/hints');
    await screen.findByText(FLOW.title);
    expect(commandsButton(FLOW.title)).toBeInTheDocument();
    expect(copyButton(FLOW.title)).toBeInTheDocument();
  });

  it('a missing-tool hint with the install command is an ordinary hint with the buttons', async () => {
    const missing = fx.commandsHint({
      id: `flow_control:${NIC}`,
      level: 'info',
      title: 'ethtool is not installed',
      detail: 'Flow control of enp101s0f0 cannot be checked: ethtool is missing.',
      suggestion: 'Install the package ethtool.',
      commands: ['apt install ethtool'],
    });
    await showHints([missing]);
    const item = itemOf(missing.title);
    expect(item).toHaveTextContent(missing.detail);
    expect(item).toHaveTextContent(missing.suggestion);
    const user = userEvent.setup();
    await user.hover(commandsButton(missing.title));
    expect(await screen.findByRole('tooltip')).toHaveTextContent('apt install ethtool');
  });

  it('a kernel-command-line hint (parameters in the detail, no commands) shows no button', async () => {
    const cmdline: Hint = {
      id: 'isolation',
      level: 'error',
      title: 'No core isolation',
      detail: 'isolcpus is not set. Add isolcpus=2-5 nohz_full=2-5 rcu_nocbs=2-5 to the kernel command line.',
      suggestion: 'Edit the boot loader configuration and reboot.',
    };
    await showHints([cmdline, SWAP]);
    expect(within(itemOf(cmdline.title)).queryAllByRole('button')).toEqual([]);
  });

  it('title, detail and suggestion of a hint with commands are still shown', async () => {
    await showHints([FIRMWARE]);
    const item = itemOf(FIRMWARE.title);
    expect(item).toHaveTextContent(FIRMWARE.detail);
    expect(item).toHaveTextContent(FIRMWARE.suggestion);
  });

  it('the commands are not printed on the page until the tooltip opens', async () => {
    await showHints([FIRMWARE]);
    expect(pageOf()).not.toHaveTextContent('mstconfig');
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
  });
});

describe('Host hints: the commands tooltip', () => {
  it('hover shows one tooltip with every command, each in a monospace block, and the note', async () => {
    const user = userEvent.setup();
    await showHints([FIRMWARE, SWAP]);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    await user.hover(commandsButton(FIRMWARE.title));
    const tip = await screen.findByRole('tooltip');
    expect(screen.getAllByRole('tooltip')).toHaveLength(1);
    for (const c of must(FIRMWARE.commands)) expect(tip).toHaveTextContent(c);
    expectMonospace(tip, FIRMWARE);
    expect(tip).toHaveTextContent(NOTE);
  });

  it('keyboard focus shows the same tooltip, blur hides it', async () => {
    await showHints([IRQ]);
    const button = commandsButton(IRQ.title);
    act(() => button.focus());
    const tip = await screen.findByRole('tooltip');
    for (const c of must(IRQ.commands)) expect(tip).toHaveTextContent(c);
    expectMonospace(tip, IRQ);
    expect(tip).toHaveTextContent(NOTE);
    act(() => button.blur());
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('tabbing to the button with the keyboard opens the tooltip', async () => {
    const user = userEvent.setup();
    await showHints([SWAP]);
    const button = commandsButton(SWAP.title);
    for (let i = 0; i < 60 && document.activeElement !== button; i++) await user.tab();
    expect(button).toHaveFocus();
    expect(await screen.findByRole('tooltip')).toHaveTextContent('sysctl -w vm.swappiness=0');
  });

  it('moving the pointer away hides the tooltip', async () => {
    const user = userEvent.setup();
    await showHints([SWAP]);
    const button = commandsButton(SWAP.title);
    await user.hover(button);
    await screen.findByRole('tooltip');
    await user.unhover(button);
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  });

  it('the tooltip belongs to its hint: another hint shows only its own commands', async () => {
    const user = userEvent.setup();
    await showHints([SWAP, FLOW, FIRMWARE]);
    await user.hover(commandsButton(FLOW.title));
    const tip = await screen.findByRole('tooltip');
    expect(tip).toHaveTextContent('ethtool -A enp101s0f0 rx off tx off');
    expect(tip).not.toHaveTextContent('sysctl');
    expect(tip).not.toHaveTextContent('mstconfig');
    await user.unhover(commandsButton(FLOW.title));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    await user.hover(commandsButton(SWAP.title));
    const second = await screen.findByRole('tooltip');
    expect(second).toHaveTextContent('sysctl -w vm.swappiness=0');
    expect(second).not.toHaveTextContent('ethtool');
  });

  it('a single command is shown too, and shell characters (> < & quotes) appear as typed', async () => {
    const user = userEvent.setup();
    const odd = fx.commandsHint({
      commands: [`echo <n> > /sys/kernel/mm/hugepages/hugepages-1048576kB/nr_hugepages && echo "it's"`],
    });
    await showHints([odd]);
    await user.hover(commandsButton(odd.title));
    const tip = await screen.findByRole('tooltip');
    expectMonospace(tip, odd);
    expect(
      within(tip)
        .getByText(/nr_hugepages/)
        .closest('code, pre'),
    ).not.toBeNull();
  });

  it('every command is its own line: no command text is split across or merged with another', async () => {
    const user = userEvent.setup();
    await showHints([FIRMWARE]);
    await user.hover(commandsButton(FIRMWARE.title));
    const tip = await screen.findByRole('tooltip');
    const [first, second] = must(FIRMWARE.commands);
    const text = tip.textContent ?? '';
    expect(text).toContain(must(first));
    expect(text).toContain(must(second));
    expect(text.indexOf(must(first))).toBeLessThan(text.indexOf(must(second))); // order kept
    expect(text).not.toContain(`${first}${second}`);
  });

  it('opening the tooltip sends nothing, opens no dialog and writes no local document', async () => {
    const user = userEvent.setup();
    const doc = seedStore({ links: [fx.link()] });
    const before = localStorage.getItem('dpds.config.v1');
    api.getHostHintsFor.mockResolvedValue([FLOW]);
    renderApp('/hints');
    await screen.findByText(FLOW.title);
    const calls = [api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length];
    await user.hover(commandsButton(FLOW.title));
    await screen.findByRole('tooltip');
    await user.click(commandsButton(FLOW.title));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect([api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length]).toEqual(calls);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
    expect(JSON.parse(String(before))).toEqual(doc);
  });
});

describe('Host hints: Copy commands', () => {
  it('writes the commands joined by newlines, once, without a trailing newline', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([FIRMWARE]);
    await user.click(copyButton(FIRMWARE.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith(must(FIRMWARE.commands).join('\n'));
    expect(String(must(writeText.mock.calls[0])[0])).not.toMatch(/\n$/);
  });

  it('a single command is copied as it is', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([SWAP]);
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith('sysctl -w vm.swappiness=0');
  });

  it('copies the commands verbatim, shell characters and spacing untouched', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    const odd = fx.commandsHint({
      commands: [
        'echo 2-15 > /proc/irq/123/smp_affinity_list',
        `dcb buffer set dev ens16np0 buffer-size all:0 1:1048576`,
        `echo "a  b" && echo 'c'`,
      ],
    });
    await showHints([odd]);
    await user.click(copyButton(odd.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith(must(odd.commands).join('\n'));
  });

  it('each hint copies its own commands, nothing of the others', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([SWAP, FLOW, IRQ]);
    await user.click(copyButton(IRQ.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenLastCalledWith(must(IRQ.commands).join('\n'));
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
    expect(writeText).toHaveBeenLastCalledWith('sysctl -w vm.swappiness=0');
    await user.click(copyButton(FLOW.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(3));
    expect(writeText).toHaveBeenLastCalledWith('ethtool -A enp101s0f0 rx off tx off');
  });

  it('the button works without the tooltip being open, and with it open', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([SWAP]);
    expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    await user.hover(commandsButton(SWAP.title));
    await screen.findByRole('tooltip');
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2));
  });

  it('copying only touches the clipboard: no request, no dialog, no local document written', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    const doc = seedStore({ links: [fx.link()] });
    const before = localStorage.getItem('dpds.config.v1');
    api.getHostHintsFor.mockResolvedValue([FLOW]);
    renderApp('/hints');
    await screen.findByText(FLOW.title);
    const calls = [api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length];
    const listed = screen.getAllByRole('listitem').length;
    await user.click(copyButton(FLOW.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect([api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length]).toEqual(calls);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).not.toHaveBeenCalled();
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
    expect(JSON.parse(String(before))).toEqual(doc);
    expect(screen.getAllByRole('listitem')).toHaveLength(listed); // the hints stay as they were
  });

  it('keyboard activation (Enter on the focused button) copies too', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([SWAP]);
    act(() => copyButton(SWAP.title).focus());
    await user.keyboard('{Enter}');
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText).toHaveBeenCalledWith('sysctl -w vm.swappiness=0');
  });

  it('a rejected clipboard write does not break the page (no unhandled rejection, hints stay)', async () => {
    const user = userEvent.setup();
    // A plain function: vi.fn() marks the promises it returns as handled, which would hide the very bug under test.
    let calls = 0;
    setClipboard(() => {
      calls++;
      return Promise.reject(new DOMException('denied', 'NotAllowedError'));
    });
    const unhandled: unknown[] = [];
    const onUnhandled = (e: unknown) => unhandled.push(e);
    process.on('unhandledRejection', onUnhandled);
    try {
      await showHints([SWAP, ...fx.hints()]);
      await user.click(copyButton(SWAP.title));
      await waitFor(() => expect(calls).toBe(1));
      await act(async () => {
        await new Promise((r) => setTimeout(r, 20));
      });
    } finally {
      process.off('unhandledRejection', onUnhandled);
    }
    expect(unhandled).toEqual([]);
    expect(screen.getAllByRole('listitem')).toHaveLength(1 + fx.hints().length);
    expect(copyButton(SWAP.title)).toBeEnabled();
  });

  it('without a clipboard (plain http on a LAN address has none) a click throws nothing and the page stays', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    const errors: unknown[] = [];
    const onError = (e: ErrorEvent) => {
      errors.push(e.error);
      e.preventDefault();
    };
    window.addEventListener('error', onError);
    try {
      await showHints([SWAP, ...fx.hints()]);
      await user.click(copyButton(SWAP.title));
      await act(async () => {
        await new Promise((r) => setTimeout(r, 20));
      });
    } finally {
      window.removeEventListener('error', onError);
    }
    expect(errors).toEqual([]);
    expect(screen.getAllByRole('listitem')).toHaveLength(1 + fx.hints().length);
    expect(commandsButton(SWAP.title)).toBeInTheDocument(); // the tooltip still lets the user read the commands
  });
});

/** The `pre` blocks of a hint's list item that are not part of a tooltip: the fallback with the commands. */
const fallbackBlocks = (title: string) =>
  Array.from(itemOf(title).querySelectorAll('pre')).filter((p) => !p.closest('[role="tooltip"]'));
/** Asserts that the hint shows the fallback: one selectable pre with exactly its commands, plus the explanation. */
function expectFallback(hint: Hint) {
  const blocks = fallbackBlocks(hint.title);
  expect(blocks, 'one inline pre block with the commands').toHaveLength(1);
  const pre = must(blocks[0]);
  expect(pre.textContent).toBe(must(hint.commands).join('\n'));
  for (let e: Element | null = pre; e && e !== document.body; e = e.parentElement) {
    expect(getComputedStyle(e).userSelect, 'the block is selectable').not.toBe('none');
    expect(e.getAttribute('aria-hidden'), 'the block is not hidden').not.toBe('true');
    expect(e.hasAttribute('hidden'), 'the block is not hidden').toBe(false);
  }
  const text = within(itemOf(hint.title)).getByText(FALLBACK_TEXT);
  expect(text).not.toBe(pre);
  expect(pre).not.toContainElement(text);
  expect(within(itemOf(hint.title)).queryByText('Copied')).toBeNull();
}
const expectNoFallback = (title: string) => {
  expect(fallbackBlocks(title)).toEqual([]);
  expect(within(itemOf(title)).queryByText(FALLBACK_TEXT)).toBeNull();
};
const copiedIn = (title: string) => within(itemOf(title)).queryByText('Copied');
const flush = () => act(async () => {});

describe('Host hints: the commands tooltip is wide and wraps (spec 33b)', () => {
  it('the tooltip element carries the class commands-tip, on hover and on focus', async () => {
    const user = userEvent.setup();
    await showHints([FIRMWARE]);
    await user.hover(commandsButton(FIRMWARE.title));
    const tip = await screen.findByRole('tooltip');
    expect(tip).toHaveClass('commands-tip');
    expect(tip).toHaveTextContent(must(must(FIRMWARE.commands)[0]));
    await user.unhover(commandsButton(FIRMWARE.title));
    await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
    act(() => commandsButton(FIRMWARE.title).focus());
    expect(await screen.findByRole('tooltip')).toHaveClass('commands-tip');
  });

  it('the commands stay in a pre inside that tooltip, so the stylesheet rule for its pre applies', async () => {
    const user = userEvent.setup();
    await showHints([IRQ]);
    await user.hover(commandsButton(IRQ.title));
    const tip = await screen.findByRole('tooltip');
    const pre = must(tip.querySelector('pre'), 'a pre in the tooltip');
    expect(pre.textContent).toBe(must(IRQ.commands).join('\n'));
  });
});

describe('Host hints: Copy commands without a usable clipboard reveals the commands (spec 33b)', () => {
  it('shows nothing of the fallback before Copy is clicked', async () => {
    setClipboard(undefined);
    await showHints([SWAP, FLOW]);
    expectNoFallback(SWAP.title);
    expectNoFallback(FLOW.title);
    expect(screen.queryByText(FALLBACK_TEXT)).toBeNull();
  });

  it('no clipboard at all: the click reveals one selectable pre with the commands and the explanation', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([FIRMWARE]);
    await user.click(copyButton(FIRMWARE.title));
    expectFallback(FIRMWARE);
    expect(within(itemOf(FIRMWARE.title)).queryByRole('status', { name: /copied/i })).toBeNull();
  });

  it('a single command is revealed too, verbatim with shell characters', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    const odd = fx.commandsHint({ commands: [`echo "a  b" > /sys/x && echo 'c' <d>`] });
    await showHints([odd]);
    await user.click(copyButton(odd.title));
    expectFallback(odd);
  });

  it('a rejected writeText reveals the same fallback (and no unhandled rejection)', async () => {
    const user = userEvent.setup();
    let calls = 0;
    // A plain function: vi.fn() would mark the returned promise as handled.
    setClipboard(() => {
      calls++;
      return Promise.reject(new DOMException('denied', 'NotAllowedError'));
    });
    const unhandled: unknown[] = [];
    const onUnhandled = (e: unknown) => unhandled.push(e);
    process.on('unhandledRejection', onUnhandled);
    try {
      await showHints([IRQ]);
      await user.click(copyButton(IRQ.title));
      await waitFor(() => expect(calls).toBe(1));
      await screen.findByText(FALLBACK_TEXT);
      await flush();
    } finally {
      process.off('unhandledRejection', onUnhandled);
    }
    expect(unhandled).toEqual([]);
    expectFallback(IRQ);
  });

  it('a writeText that throws synchronously reveals the fallback too', async () => {
    const user = userEvent.setup();
    setClipboard(() => {
      throw new DOMException('not allowed', 'SecurityError');
    });
    await showHints([SWAP]);
    await user.click(copyButton(SWAP.title));
    expectFallback(SWAP);
  });

  it('the fallback belongs to its hint: the others show nothing', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([SWAP, FLOW, IRQ]);
    await user.click(copyButton(FLOW.title));
    expectFallback(FLOW);
    expectNoFallback(SWAP.title);
    expectNoFallback(IRQ.title);
    expect(screen.getAllByText(FALLBACK_TEXT)).toHaveLength(1);
    await user.click(copyButton(IRQ.title));
    expectFallback(IRQ);
    expectFallback(FLOW);
    expectNoFallback(SWAP.title);
  });

  it('a second click keeps the fallback revealed (one block, not toggled away or doubled)', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([FIRMWARE]);
    await user.click(copyButton(FIRMWARE.title));
    await user.click(copyButton(FIRMWARE.title));
    await flush();
    expectFallback(FIRMWARE);
  });

  it('keyboard activation reveals it too, and the tooltip still opens next to it', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([IRQ]);
    act(() => copyButton(IRQ.title).focus());
    await user.keyboard('{Enter}');
    expectFallback(IRQ);
    await user.hover(commandsButton(IRQ.title));
    const tip = await screen.findByRole('tooltip');
    for (const c of must(IRQ.commands)) expect(tip).toHaveTextContent(c);
    expectFallback(IRQ); // the one block outside the tooltip is still just one
  });

  it('revealing sends no request, opens no dialog and writes no local document', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    const doc = seedStore({ links: [fx.link()] });
    const before = localStorage.getItem('dpds.config.v1');
    api.getHostHintsFor.mockResolvedValue([FLOW]);
    renderApp('/hints');
    await screen.findByText(FLOW.title);
    const calls = [api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length];
    await user.click(copyButton(FLOW.title));
    expectFallback(FLOW);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect([api.getHostHints.mock.calls.length, api.getHostHintsFor.mock.calls.length]).toEqual(calls);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
    expect(JSON.parse(String(before))).toEqual(doc);
  });

  it('a working clipboard reveals no fallback', async () => {
    const user = userEvent.setup();
    const writeText = mockClipboard();
    await showHints([SWAP]);
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    await flush();
    expectNoFallback(SWAP.title);
  });

  it('the clipboard is read at click time: it can appear after the page loaded and is then used', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([SWAP]);
    const writeText = mockClipboard();
    await user.click(copyButton(SWAP.title));
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expectNoFallback(SWAP.title);
  });
});

describe('Host hints: Copy commands confirms a successful copy (spec 33b)', () => {
  it('shows no Copied status before the click', async () => {
    mockClipboard();
    await showHints([SWAP, FLOW]);
    expect(screen.queryByText('Copied')).toBeNull();
  });

  it('shows a role=status "Copied" in the hint after the write resolved, and hides it after about 2 s', async () => {
    mockClipboard();
    await showHints([SWAP]);
    vi.useFakeTimers();
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    const copied = must(copiedIn(SWAP.title), 'the Copied text in the hint');
    expect(copied.closest('[role="status"]'), 'inside a role=status element').not.toBeNull();
    expect(copied.textContent?.trim()).toBe('Copied');
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(copiedIn(SWAP.title), 'still shown after 1 s').not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(copiedIn(SWAP.title), 'gone after 3 s').toBeNull();
    expectNoFallback(SWAP.title);
  });

  it('shows Copied only once the write has resolved, not while it is pending', async () => {
    let release: () => void = () => {};
    const pending = new Promise<void>((r) => {
      release = r;
    });
    setClipboard(() => pending);
    await showHints([SWAP]);
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    expect(copiedIn(SWAP.title)).toBeNull();
    expectNoFallback(SWAP.title);
    release();
    await flush();
    expect(copiedIn(SWAP.title)).not.toBeNull();
  });

  it('a second copy restarts the 2 s: an old timer does not hide the new confirmation early', async () => {
    mockClipboard();
    await showHints([SWAP]);
    vi.useFakeTimers();
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    act(() => {
      vi.advanceTimersByTime(1500);
    });
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    act(() => {
      vi.advanceTimersByTime(1000); // 2.5 s after the first copy, 1 s after the second
    });
    expect(copiedIn(SWAP.title), 'the first timer must not hide it').not.toBeNull();
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(copiedIn(SWAP.title)).toBeNull();
  });

  it('copying again after it disappeared shows it again', async () => {
    mockClipboard();
    await showHints([SWAP]);
    vi.useFakeTimers();
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(copiedIn(SWAP.title)).toBeNull();
    fireEvent.click(copyButton(SWAP.title));
    await flush();
    expect(copiedIn(SWAP.title)).not.toBeNull();
  });

  it('the confirmation belongs to the hint that was copied', async () => {
    mockClipboard();
    await showHints([SWAP, FLOW]);
    fireEvent.click(copyButton(FLOW.title));
    await flush();
    expect(copiedIn(FLOW.title)).not.toBeNull();
    expect(copiedIn(SWAP.title)).toBeNull();
    expect(screen.getAllByText('Copied')).toHaveLength(1);
  });

  it('a failed copy shows no Copied, also not for a moment', async () => {
    setClipboard(() => Promise.reject(new DOMException('denied', 'NotAllowedError')));
    await showHints([SWAP]);
    fireEvent.click(copyButton(SWAP.title));
    expect(copiedIn(SWAP.title)).toBeNull();
    await screen.findByText(FALLBACK_TEXT);
    expect(copiedIn(SWAP.title)).toBeNull();
  });

  it('leaving the page within the 2 s throws nothing', async () => {
    mockClipboard();
    const errors: unknown[] = [];
    const onError = (e: ErrorEvent) => {
      errors.push(e.error);
      e.preventDefault();
    };
    window.addEventListener('error', onError);
    try {
      const { unmount } = await showHints([SWAP]);
      vi.useFakeTimers();
      fireEvent.click(copyButton(SWAP.title));
      await flush();
      unmount();
      act(() => {
        vi.advanceTimersByTime(5000);
      });
    } finally {
      window.removeEventListener('error', onError);
    }
    expect(errors).toEqual([]);
  });
});

describe('Host hints: DPDS applies nothing', () => {
  const FORBIDDEN = /apply|undo|revert|reset|roll ?back|restore|turn .*off|untune|execute|confirm/i;
  const controls = () => [...within(pageOf()).queryAllByRole('button'), ...within(pageOf()).queryAllByRole('link')];
  const nameOf = (c: HTMLElement) => c.getAttribute('aria-label') ?? c.textContent ?? '';

  it('no Apply button, no confirm dialog and no apply/undo-like control, before and after the interactions', async () => {
    const user = userEvent.setup();
    mockClipboard();
    await showHints([SWAP, FLOW, FIRMWARE, IRQ, ...fx.hints()]);
    const names = () => controls().map(nameOf);
    expect(names().filter((n) => FORBIDDEN.test(n))).toEqual([]);
    expect(screen.queryAllByRole('button', { name: /^apply/i })).toEqual([]);
    await user.hover(commandsButton(FIRMWARE.title));
    await screen.findByRole('tooltip');
    await user.click(copyButton(FIRMWARE.title));
    await user.click(commandsButton(IRQ.title));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(names().filter((n) => FORBIDDEN.test(n))).toEqual([]);
    expect(screen.queryByRole('button', { name: FORBIDDEN })).not.toBeInTheDocument();
  });

  it('a hint with the revealed fallback has no apply-like control either (spec 33b)', async () => {
    const user = userEvent.setup();
    setClipboard(undefined);
    await showHints([FIRMWARE, ...fx.hints()]);
    await user.click(copyButton(FIRMWARE.title));
    expectFallback(FIRMWARE);
    const names = within(itemOf(FIRMWARE.title)).getAllByRole('button').map(nameOf);
    expect(names.filter((n) => FORBIDDEN.test(n))).toEqual([]);
    expect(names).toContain('Copy commands');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('a hint carries nothing but the two commands buttons: no third control', async () => {
    await showHints([FIRMWARE]);
    const names = within(itemOf(FIRMWARE.title)).getAllByRole('button').map(nameOf);
    expect(names.sort()).toEqual([`Commands for ${FIRMWARE.title}`, 'Copy commands'].sort());
  });

  it('a hint that still carries an old `action` field gets no Apply button (the field is gone from the contract)', async () => {
    const legacy = {
      ...must(fx.hints()[1]),
      action: { id: 'swappiness', label: 'Apply', confirm: 'Writes 0 to /proc/sys/vm/swappiness.' },
    } as Hint;
    await showHints([legacy]);
    expect(within(itemOf(legacy.title)).queryAllByRole('button')).toEqual([]);
  });

  it('the page does not read the emulation state: no getEmulation call, nothing disabled, no emulation text', async () => {
    api.getEmulation.mockResolvedValue(fx.emulation('running'));
    seedStore({ links: [fx.link()] });
    api.getHostHintsFor.mockResolvedValue([FLOW]);
    renderApp('/hints');
    await screen.findByText(FLOW.title);
    await act(async () => {}); // let a would-be state answer land
    expect(api.getEmulation).not.toHaveBeenCalled();
    expect(commandsButton(FLOW.title)).toBeEnabled();
    expect(copyButton(FLOW.title)).toBeEnabled();
    expect(pageOf().textContent).not.toMatch(/emulation/i);
    for (const e of Array.from(pageOf().querySelectorAll('[title]')))
      expect(e.getAttribute('title')).not.toMatch(/emulation/i);
  });

  it('a failing emulation state is irrelevant too: no alert from it', async () => {
    api.getEmulation.mockRejectedValue(new Error('controller down'));
    await showHints([SWAP]);
    await act(async () => {});
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(commandsButton(SWAP.title)).toBeEnabled();
  });

  it('the frontend has no applyHostAction, HintAction or /host/actions route left anywhere in its source', () => {
    const srcDir = resolve(__dirname, '..');
    const files: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name);
        if (statSync(p).isDirectory()) walk(p);
        else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name) && !p.includes(`${join('src', 'test')}`))
          files.push(p);
      }
    };
    walk(srcDir);
    expect(files.length).toBeGreaterThan(10);
    for (const f of files) {
      const text = readFileSync(f, 'utf8');
      expect(text, f).not.toMatch(/applyHostAction|HintAction|host\/actions/);
    }
    const types = readFileSync(join(srcDir, 'api', 'types.ts'), 'utf8');
    expect(types).toMatch(/commands\?:\s*string\[\]/);
    expect(types).not.toMatch(/\baction\??:/);
  });
});
