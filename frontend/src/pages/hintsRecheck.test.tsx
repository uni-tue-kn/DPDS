import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Hint } from '../api/types';
import { api, ApiError } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { must } from '../test/must';
import { expectStartBody, renderApp, seedLinks, seedRaw, seedStore } from '../test/helpers';

// Spec 34, frontend: the Host hints page shows a loading animation while it checks the host, and has a Recheck button.
//
// CONTRACT (from the spec; the choices of the tester where the spec is silent are marked CHOICE):
// - While the page's requests are in flight (the first load and every recheck) a status element is shown whose text is exactly
//   "Checking host…" (U+2026). It has the implicit status role (found with getByRole, never by attribute: Biome flags an explicit
//   role on <output>); its spinner is aria-hidden and carries no text of its own. It is gone once every request has settled.
// - A button named exactly "Recheck" with an `i.bi-arrow-clockwise` icon (aria-hidden), type="button". It is disabled while a
//   check is in flight (also during the first load). Clicking it re-sends api.getHostHints() and, when the local document has
//   links (and is not corrupt), api.getHostHintsFor(startBody, { recheck: true }). The first load passes no recheck.
// - During a recheck the hints already shown stay visible; the icon gets the class `icon-spin` (CHOICE of the class name; the
//   stylesheet side is pinned in assets/stylesSpec34.test.ts). When the requests have settled, the new answers replace the old
//   ones and the icon stops spinning. Errors show as before (an alert with the message).
// - Two clicks in quick succession send one recheck.
// NOT PINNED: whether the old hints stay on screen after a recheck that failed (useLoad drops the data of a failed reload), and
// what an old alert looks like while a recheck is in flight.

const CHECKING = 'Checking host…';

function deferred<T>() {
  const box: { resolve?: (v: T) => void; reject?: (e: unknown) => void } = {};
  const promise = new Promise<T>((resolve, reject) => {
    box.resolve = resolve;
    box.reject = reject;
  });
  return {
    promise,
    resolve: (v: T) => must(box.resolve, 'resolve')(v),
    reject: (e: unknown) => must(box.reject, 'reject')(e),
  };
}

/** The "Checking host…" statuses (the "Copied" statuses of hints with commands are other statuses). */
const checking = () => screen.queryAllByRole('status').filter((s) => (s.textContent ?? '').includes('Checking host'));
const recheckButton = () => screen.getByRole('button', { name: 'Recheck' });
const icon = () => must(recheckButton().querySelector('.bi-arrow-clockwise'), 'the .bi-arrow-clockwise icon');

const NEW_HINT: Hint = {
  id: 'fresh',
  level: 'ok',
  title: 'Fresh answer',
  detail: 'Read again.',
  suggestion: 'Nothing to do.',
};
const FROM_LINKS: Hint = {
  id: 'numa',
  level: 'warn',
  title: 'NUMA mismatch',
  detail: 'a-b crosses nodes.',
  suggestion: 'Use node 0.',
};

/** Renders the page and waits for the first answer. */
async function loaded() {
  const view = renderApp('/hints');
  await screen.findByRole('region', { name: /^Errors/ });
  await waitFor(() => expect(checking()).toHaveLength(0));
  return view;
}

describe('Host hints page: loading animation (spec 34)', () => {
  it('shows one "Checking host…" status with an aria-hidden spinner until the request resolves', async () => {
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValue(get.promise);
    renderApp('/hints');
    const [status, ...others] = checking();
    expect(others).toHaveLength(0);
    expect(must(status).textContent?.trim()).toBe(CHECKING); // the spinner has no text of its own
    const hidden = must(status).querySelector('[aria-hidden="true"]');
    expect(hidden, 'a spinner element that is aria-hidden').not.toBeNull();
    expect(hidden?.textContent ?? '').toBe('');
    expect(screen.getByRole('heading', { level: 1, name: 'Host hints' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: /^Errors/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/no hints/i)).not.toBeInTheDocument(); // not "No hints." while still checking
    await act(async () => get.resolve(fx.hints()));
    await screen.findByRole('region', { name: /^Errors/ });
    expect(checking()).toHaveLength(0);
  });

  it('the status is gone once the hints are listed, and an empty answer shows "No hints." without it', async () => {
    api.getHostHints.mockResolvedValue([]);
    renderApp('/hints');
    expect(await screen.findByText(/no hints/i)).toBeInTheDocument();
    expect(checking()).toHaveLength(0);
  });

  it('with links the status stays until BOTH requests have answered', async () => {
    seedLinks([fx.link()]);
    const post = deferred<Hint[]>();
    api.getHostHintsFor.mockReturnValue(post.promise);
    renderApp('/hints');
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(1));
    await act(async () => {}); // the GET has answered by now
    expect(checking(), 'the POST is still in flight').toHaveLength(1);
    await act(async () => post.resolve([FROM_LINKS]));
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(screen.getByRole('region', { name: /^Warnings/ })).toHaveTextContent('NUMA mismatch');
  });

  it('the status is also gone after failed requests, which show their alert', async () => {
    seedLinks([fx.link()]);
    api.getHostHints.mockRejectedValue(new ApiError(500, 'procfs unreadable'));
    api.getHostHintsFor.mockRejectedValue(new ApiError(409, 'port is a host uplink'));
    renderApp('/hints');
    await waitFor(() => expect(screen.getAllByRole('alert')).toHaveLength(2));
    expect(checking()).toHaveLength(0);
  });
});

describe('Host hints page: the Recheck button (spec 34)', () => {
  it('is named Recheck, has an aria-hidden bi-arrow-clockwise icon, is a plain button and is enabled once loaded', async () => {
    await loaded();
    const button = recheckButton();
    expect(button).toHaveAttribute('type', 'button');
    expect(button).toBeEnabled();
    expect(icon()).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getAllByRole('button', { name: 'Recheck' })).toHaveLength(1);
    expect(icon().classList.contains('icon-spin'), 'idle: the icon does not spin').toBe(false);
  });

  it('is there and disabled during the first load, and a click on it sends nothing', async () => {
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValue(get.promise);
    renderApp('/hints');
    expect(recheckButton()).toBeDisabled();
    fireEvent.click(recheckButton());
    await act(async () => {});
    expect(api.getHostHints).toHaveBeenCalledTimes(1);
    await act(async () => get.resolve(fx.hints()));
    await waitFor(() => expect(recheckButton()).toBeEnabled());
  });

  it('without local links a click re-sends the GET only (no POST), as a plain call', async () => {
    const user = userEvent.setup();
    await loaded();
    await user.click(recheckButton());
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(2));
    expect(api.getHostHints.mock.calls[1]).toEqual([]);
    await act(async () => {});
    expect(api.getHostHintsFor).not.toHaveBeenCalled();
  });

  it('the first load sends no recheck; a click sends getHostHintsFor(start body, { recheck: true }) next to the GET', async () => {
    const user = userEvent.setup();
    const doc = seedStore({ links: [fx.link(), fx.linkRev()], settings: fx.settings({ poll_interval_ms: 250 }) });
    api.getHostHintsFor.mockResolvedValue([FROM_LINKS]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Warnings/ });
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(api.getHostHints).toHaveBeenCalledTimes(1);
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(1);
    expect(api.getHostHintsFor.mock.calls[0][1]?.recheck ?? false, 'the first load does not recheck').toBe(false);

    await user.click(recheckButton());
    await waitFor(() => expect(api.getHostHintsFor).toHaveBeenCalledTimes(2));
    expect(api.getHostHints).toHaveBeenCalledTimes(2);
    expectStartBody(api.getHostHintsFor.mock.calls[1][0], doc);
    expect(api.getHostHintsFor.mock.calls[1][1]).toEqual({ recheck: true });
    expect(api.getHostHints.mock.calls[1]).toEqual([]);
  });

  it('a corrupt local document has no links to send: a click re-sends the GET only', async () => {
    const user = userEvent.setup();
    seedRaw('{ not json');
    await loaded();
    await user.click(recheckButton());
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(2));
    await act(async () => {});
    expect(api.getHostHintsFor).not.toHaveBeenCalled();
  });

  it('while the recheck runs: the old hints stay, the status shows, the button is disabled and its icon spins', async () => {
    const user = userEvent.setup();
    await loaded();
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValueOnce(get.promise);
    await user.click(recheckButton());

    expect(checking()).toHaveLength(1);
    expect(must(checking()[0]).textContent?.trim()).toBe(CHECKING);
    expect(recheckButton()).toBeDisabled();
    expect(icon().classList.contains('icon-spin'), 'the icon spins').toBe(true);
    // the previous answer is still on screen
    expect(screen.getByRole('region', { name: /^Errors/ })).toHaveTextContent('No core isolation');
    expect(screen.getByRole('region', { name: /^Warnings/ })).toHaveTextContent('Few hugepages');
    expect(screen.queryByText('Fresh answer')).not.toBeInTheDocument();
    expect(screen.queryByText(/no hints/i)).not.toBeInTheDocument();
    await act(async () => get.resolve([NEW_HINT]));
    await waitFor(() => expect(checking()).toHaveLength(0));
  });

  it('after the recheck the new hints replace the old ones, the status is gone and the button is usable again', async () => {
    const user = userEvent.setup();
    await loaded();
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValueOnce(get.promise);
    await user.click(recheckButton());
    await act(async () => get.resolve([NEW_HINT]));

    expect(await screen.findByText('Fresh answer')).toBeInTheDocument();
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(screen.queryByText('No core isolation')).not.toBeInTheDocument();
    expect(screen.queryByText('Few hugepages')).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: /^Errors/ })).not.toBeInTheDocument();
    expect(recheckButton()).toBeEnabled();
    expect(icon().classList.contains('icon-spin')).toBe(false);
  });

  it('with links the recheck is over only when the GET and the POST have both answered', async () => {
    const user = userEvent.setup();
    seedLinks([fx.link()]);
    api.getHostHintsFor.mockResolvedValue([FROM_LINKS]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Warnings/ });
    await waitFor(() => expect(checking()).toHaveLength(0));

    const get = deferred<Hint[]>();
    const post = deferred<Hint[]>();
    api.getHostHints.mockReturnValueOnce(get.promise);
    api.getHostHintsFor.mockReturnValueOnce(post.promise);
    await user.click(recheckButton());
    expect(recheckButton()).toBeDisabled();

    await act(async () => get.resolve([NEW_HINT]));
    expect(checking(), 'the POST is still in flight').toHaveLength(1);
    expect(recheckButton()).toBeDisabled();
    expect(icon().classList.contains('icon-spin')).toBe(true);

    await act(async () => post.resolve([{ ...FROM_LINKS, title: 'NUMA fixed', level: 'ok' }]));
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(recheckButton()).toBeEnabled();
    expect(screen.getByText('NUMA fixed')).toBeInTheDocument();
    expect(screen.getByText('Fresh answer')).toBeInTheDocument();
    expect(screen.queryByText('NUMA mismatch')).not.toBeInTheDocument();
    expect(screen.queryByText('No core isolation')).not.toBeInTheDocument();
  });

  it('a failing recheck shows its message in an alert, ends the animation and enables the button again', async () => {
    const user = userEvent.setup();
    await loaded();
    api.getHostHints.mockRejectedValueOnce(new ApiError(500, 'procfs unreadable'));
    await user.click(recheckButton());
    expect(await screen.findByRole('alert')).toHaveTextContent('procfs unreadable');
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(recheckButton()).toBeEnabled();
    expect(icon().classList.contains('icon-spin')).toBe(false);
  });

  it('a failing POST of the recheck shows its message while the new host hints are listed', async () => {
    const user = userEvent.setup();
    seedLinks([fx.link()]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    await waitFor(() => expect(checking()).toHaveLength(0));
    api.getHostHints.mockResolvedValueOnce([NEW_HINT]);
    api.getHostHintsFor.mockRejectedValueOnce(new ApiError(409, 'port 0000:17:00.0 is a host uplink'));
    await user.click(recheckButton());
    expect(await screen.findByRole('alert')).toHaveTextContent('port 0000:17:00.0 is a host uplink');
    expect(await screen.findByText('Fresh answer')).toBeInTheDocument();
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(recheckButton()).toBeEnabled();
  });

  it('a successful recheck clears the alert of a failed first load', async () => {
    const user = userEvent.setup();
    api.getHostHints.mockRejectedValueOnce(new ApiError(500, 'procfs unreadable'));
    renderApp('/hints');
    expect(await screen.findByRole('alert')).toHaveTextContent('procfs unreadable');
    await waitFor(() => expect(recheckButton()).toBeEnabled());
    await user.click(recheckButton());
    await screen.findByRole('region', { name: /^Errors/ });
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(checking()).toHaveLength(0);
  });

  it('two clicks in the same instant send one recheck (GET and POST once each)', async () => {
    seedLinks([fx.link()]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    await waitFor(() => expect(checking()).toHaveLength(0));
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValueOnce(get.promise);
    const button = recheckButton();
    fireEvent.click(button);
    fireEvent.click(button);
    await act(async () => {});
    expect(api.getHostHints).toHaveBeenCalledTimes(2); // the first load plus ONE recheck
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(2);
    await act(async () => get.resolve([NEW_HINT]));
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(api.getHostHints).toHaveBeenCalledTimes(2);
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(2);
  });

  it('a double click (user-event) sends one recheck', async () => {
    const user = userEvent.setup();
    seedLinks([fx.link()]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    await waitFor(() => expect(checking()).toHaveLength(0));
    const get = deferred<Hint[]>();
    api.getHostHints.mockReturnValueOnce(get.promise);
    await user.dblClick(recheckButton());
    await act(async () => get.resolve([NEW_HINT]));
    await waitFor(() => expect(checking()).toHaveLength(0));
    expect(api.getHostHints).toHaveBeenCalledTimes(2);
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(2);
    expect(must(api.getHostHintsFor.mock.calls[1])[1]).toEqual({ recheck: true });
  });

  it('once a recheck has finished, the next click rechecks again', async () => {
    const user = userEvent.setup();
    seedLinks([fx.link()]);
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    await waitFor(() => expect(checking()).toHaveLength(0));
    await user.click(recheckButton());
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(recheckButton()).toBeEnabled());
    await user.click(recheckButton());
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(3));
    expect(api.getHostHintsFor).toHaveBeenCalledTimes(3);
    expect(must(api.getHostHintsFor.mock.calls[2])[1]).toEqual({ recheck: true });
    await waitFor(() => expect(recheckButton()).toBeEnabled());
  });

  it('rechecking reads the local document but never writes it, and starts or asks for nothing else', async () => {
    const user = userEvent.setup();
    seedLinks([fx.link()]);
    const before = localStorage.getItem('dpds.config.v1');
    renderApp('/hints');
    await screen.findByRole('region', { name: /^Errors/ });
    await user.click(recheckButton());
    await waitFor(() => expect(recheckButton()).toBeEnabled());
    await waitFor(() => expect(api.getHostHints).toHaveBeenCalledTimes(2));
    expect(localStorage.getItem('dpds.config.v1')).toBe(before);
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).not.toHaveBeenCalled();
    expect(api.getInterfaces).not.toHaveBeenCalled();
  });

  it('the page has no other button than Recheck while it lists hints without commands', async () => {
    await loaded();
    const page = must(screen.getByRole('heading', { level: 1, name: 'Host hints' }).parentElement, 'the page');
    expect(
      within(page)
        .getAllByRole('button')
        .map((b) => b.textContent),
    ).toEqual(['Recheck']);
  });
});
