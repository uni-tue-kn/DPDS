// Spec 59 F1-F4: App swaps the routed page for P4TG's Offline page while the controller is unreachable.
// - F1: the Sidebar's `/api/online` poll rejects with a non-ApiError (network failure) -> offline. An ApiError (any HTTP status) does not.
// - F2: `<main>` shows, instead of the routed page, a Row.text-center holding `<i class="bi bi-wifi-off" aria-hidden>` (fontSize 100px,
//   marginTop calc(30vh)) and `<p>DPDS device not reachable</p>`. Footer, sidebar ("Offline") and the alerts above stay.
// - F3: while offline `api.getOnline()` is polled every 2000 ms, first call at once; a resolved answer returns to the routed page at the same URL.
//   (The Sidebar polls every 5000 ms too: call counts below are taken in windows that end before its next poll at t = 5000.)
// - F4: online until the first poll settles (no flash of the offline page).
import { screen, within } from '@testing-library/react';
import * as fx from './test/fixtures';
import { deferred, renderApp, seedRaw, tick } from './test/helpers';
import { api, ApiError } from './test/mockApi';
import { must } from './test/must';

const TEXT = 'DPDS device not reachable';
const icon = (container: HTMLElement) => container.querySelector('i.bi-wifi-off');
const hintsHeading = () => screen.queryByRole('heading', { name: 'Host hints' });

/** getOnline fails with a network error while `up` is false, answers fx.online() otherwise. */
function controller(up = false) {
  const state = { up };
  api.getOnline.mockImplementation(() =>
    state.up ? Promise.resolve(fx.online()) : Promise.reject(new Error('Failed to fetch')),
  );
  return state;
}

beforeEach(() => {
  vi.useFakeTimers();
});

describe('offline page (spec 59 F1, F2)', () => {
  it.each([
    ['Error', new Error('Failed to fetch')],
    ['TypeError', new TypeError('Failed to fetch')],
  ])('replaces the routed page when the online poll fails with a %s', async (_name, error) => {
    api.getOnline.mockRejectedValue(error);
    const { container } = renderApp('/hints');
    await tick(0);
    const main = must(container.querySelector('main'), 'the <main> page area');
    expect(within(main).getByText(TEXT)).toBeInTheDocument();
    expect(icon(container)).toBeInTheDocument();
    expect(hintsHeading()).not.toBeInTheDocument();
  });

  it('renders P4TG markup: Row.text-center with the wifi-off icon (100px, 30vh) and the paragraph', async () => {
    controller(false);
    const { container } = renderApp('/hints');
    await tick(0);
    const main = must(container.querySelector('main'), 'the <main> page area');
    const el = must(main.querySelector('i.bi.bi-wifi-off'), 'the wifi-off icon inside <main>') as HTMLElement;
    expect(el.getAttribute('aria-hidden')).toBe('true');
    expect(el.style.fontSize).toBe('100px');
    expect(el.style.marginTop).toBe('calc(30vh)');
    const text = within(main).getByText(TEXT);
    expect(text.tagName).toBe('P');
    expect(text.textContent).toBe(TEXT);
    const row = must(text.closest('.row'), 'the Row around the paragraph');
    expect(row).toHaveClass('text-center');
    expect(row.contains(el)).toBe(true);
  });

  it('hides every routed page, not only /hints', async () => {
    controller(false);
    const { container } = renderApp('/settings');
    await tick(0);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Settings' })).not.toBeInTheDocument();
    expect(container.querySelector('main')).not.toBeEmptyDOMElement();
  });

  it('is not shown for an ApiError (HTTP 500) of the online poll', async () => {
    api.getOnline.mockRejectedValue(new ApiError(500, 'boom'));
    const { container } = renderApp('/hints');
    await tick(0);
    await tick(5500);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(icon(container)).not.toBeInTheDocument();
    expect(hintsHeading()).toBeInTheDocument();
    expect(screen.getByText('Offline')).toBeInTheDocument();
  });

  it('is not shown while the controller answers', async () => {
    controller(true);
    const { container } = renderApp('/hints');
    await tick(0);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(icon(container)).not.toBeInTheDocument();
    expect(hintsHeading()).toBeInTheDocument();
  });

  it('goes offline when a later poll fails, keeping sidebar, footer and the corrupt-config alert', async () => {
    seedRaw('not json');
    const state = controller(true);
    const { container } = renderApp('/hints');
    await tick(0);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    state.up = false;
    await tick(5500);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /DPDS @ GitHub/ })).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(must(container.querySelector('main')).contains(screen.getByRole('link', { name: /DPDS @ GitHub/ }))).toBe(
      true,
    );
  });
});

describe('offline polling and recovery (spec 59 F3)', () => {
  it('calls getOnline again at once (Sidebar + offline page) and then every 2000 ms', async () => {
    controller(false);
    renderApp('/hints');
    await tick(0);
    expect(api.getOnline).toHaveBeenCalledTimes(2);
    api.getOnline.mockClear();
    await tick(1900);
    expect(api.getOnline).toHaveBeenCalledTimes(0);
    await tick(100);
    expect(api.getOnline).toHaveBeenCalledTimes(1);
    await tick(2000);
    expect(api.getOnline).toHaveBeenCalledTimes(2);
  });

  it('returns to the routed page at the same URL after a resolved answer, without a reload', async () => {
    const state = controller(false);
    const { container } = renderApp('/hints');
    await tick(0);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
    state.up = true;
    await tick(2000);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(icon(container)).not.toBeInTheDocument();
    expect(hintsHeading()).toBeInTheDocument();
  });

  it('stops its own polling once back online (only the Sidebar poll at 5000 ms is left)', async () => {
    const state = controller(false);
    renderApp('/hints');
    await tick(0);
    state.up = true;
    await tick(2000);
    expect(hintsHeading()).toBeInTheDocument();
    api.getOnline.mockClear();
    await tick(2900);
    expect(api.getOnline).toHaveBeenCalledTimes(0);
    await tick(100);
    expect(api.getOnline).toHaveBeenCalledTimes(1);
  });

  it('keeps showing the page while the answers stay failed', async () => {
    controller(false);
    renderApp('/hints');
    await tick(0);
    await tick(10000);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
    expect(hintsHeading()).not.toBeInTheDocument();
  });
});

describe('initial state (spec 59 F4)', () => {
  it('shows the routed page, not the offline page, until the first poll settles', async () => {
    const first = deferred<ReturnType<typeof fx.online>>();
    api.getOnline.mockReturnValue(first.promise);
    const { container } = renderApp('/hints');
    await tick(0);
    expect(api.getOnline).toHaveBeenCalledTimes(1);
    expect(screen.queryByText(TEXT)).not.toBeInTheDocument();
    expect(icon(container)).not.toBeInTheDocument();
    expect(hintsHeading()).toBeInTheDocument();
    first.reject(new Error('Failed to fetch'));
    await tick(0);
    expect(screen.getByText(TEXT)).toBeInTheDocument();
    expect(hintsHeading()).not.toBeInTheDocument();
  });
});
