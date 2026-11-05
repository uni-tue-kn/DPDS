// Spec 48 F2/F3: App warns above the routed page when `/api/online` reports other connected users (P4TG's SessionWarning).
// - `connected_clients > 0`: a div[data-testid="session-warning"] above the page content with the text
//   `⚠ Currently, ${n} other user${n > 1 ? 's are' : ' is'} connected to this DPDS instance` and P4TG's inline styles.
// - Absent at 0, when the field is missing, and as soon as a poll fails (it comes back with the next good poll).
import { screen } from '@testing-library/react';
import * as fx from './test/fixtures';
import { renderApp, tick } from './test/helpers';
import { api } from './test/mockApi';
import { must } from './test/must';

const TESTID = 'session-warning';
const warning = () => screen.queryByTestId(TESTID);

beforeEach(() => {
  vi.useFakeTimers();
});

async function renderWith(connected?: number, path = '/hints') {
  api.getOnline.mockResolvedValue(connected === undefined ? fx.online() : fx.online({ connected_clients: connected }));
  const view = renderApp(path);
  await tick(0);
  return view;
}

describe('connected users warning (spec 48 F3)', () => {
  it('is shown for one other user with the singular text', async () => {
    await renderWith(1);
    expect(must(warning()).textContent).toBe('⚠ Currently, 1 other user is connected to this DPDS instance');
  });

  it('is shown for two other users with the plural text', async () => {
    await renderWith(2);
    expect(must(warning()).textContent).toBe('⚠ Currently, 2 other users are connected to this DPDS instance');
  });

  it('uses the number as reported (7)', async () => {
    await renderWith(7);
    expect(must(warning()).textContent).toBe('⚠ Currently, 7 other users are connected to this DPDS instance');
  });

  it('carries the SessionWarning inline styles of P4TG', async () => {
    await renderWith(1);
    const el = must(warning());
    expect(el.tagName).toBe('DIV');
    expect(el.style.marginRight).toBe('10px');
    expect(el.style.marginBottom).toBe('10px');
    expect(el.style.padding).toBe('5px 25px');
    expect(el.style.color).toMatch(/^(#000|#000000|rgb\(0,\s*0,\s*0\))$/);
    expect(el.style.borderRadius).toBe('10px');
    expect(el.style.textAlign).toBe('center');
    expect(el.style.display).toBe('inline-block');
    expect(el.style.background).toBe('var(--bs-warning)');
  });

  it('sits above the routed page content', async () => {
    const { container } = await renderWith(1, '/hints');
    const el = must(warning());
    const main = must(container.querySelector('main'), 'the <main> page area');
    expect(main.contains(el) || (el.compareDocumentPosition(main) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0).toBe(true);
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    expect(nav.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('is absent when no other user is connected (0)', async () => {
    await renderWith(0);
    expect(screen.getByText(/v1\.0\.0/)).toBeInTheDocument();
    expect(warning()).not.toBeInTheDocument();
  });

  it('is absent when the controller does not report the field', async () => {
    await renderWith(undefined);
    expect(screen.getByText(/v1\.0\.0/)).toBeInTheDocument();
    expect(warning()).not.toBeInTheDocument();
  });

  it('disappears after a failed poll', async () => {
    api.getOnline
      .mockResolvedValueOnce(fx.online({ connected_clients: 1 }))
      .mockRejectedValue(new Error('Failed to fetch'));
    renderApp('/hints');
    await tick(0);
    expect(warning()).toBeInTheDocument();
    await tick(5500);
    expect(screen.getByText('Offline')).toBeInTheDocument();
    expect(warning()).not.toBeInTheDocument();
  });

  it('disappears when the count drops to 0 and follows later counts', async () => {
    api.getOnline
      .mockResolvedValueOnce(fx.online({ connected_clients: 2 }))
      .mockResolvedValueOnce(fx.online({ connected_clients: 0 }))
      .mockResolvedValue(fx.online({ connected_clients: 1 }));
    renderApp('/hints');
    await tick(0);
    expect(must(warning()).textContent).toContain('2 other users are');
    await tick(5500);
    expect(warning()).not.toBeInTheDocument();
    await tick(5000);
    expect(must(warning()).textContent).toContain('1 other user is');
  });

  it('stays on every routed page', async () => {
    await renderWith(1, '/settings');
    expect(warning()).toBeInTheDocument();
  });
});
