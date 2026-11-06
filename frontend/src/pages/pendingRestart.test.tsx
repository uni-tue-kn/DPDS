import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { EmulationState, Link } from '../api/types';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { dropNulls, rawStore, renderApp, seedRaw, seedStore, storedDoc } from '../test/helpers';
import { must } from '../test/must';

// The controller stores nothing: the page compares the LOCAL document with `GET /api/emulation`.config (the running start request)
// and flags what differs.

const LINKS = () => [fx.link(), fx.linkRev()]; // a-b (id 1), b-a (id 3)
const RUNNING = (links: Link[] = LINKS()) => fx.runningConfig(links);

/** The emulation reports `config` in `state`; the local document is `links`. */
function setup(opts: { local?: Link[]; running?: ReturnType<typeof RUNNING> | null; state?: EmulationState } = {}) {
  const { local = LINKS(), running = RUNNING(), state = 'running' } = opts;
  seedStore({ links: local });
  api.getEmulation.mockResolvedValue(running === null ? fx.emulation(state) : fx.emulation(state, { config: running }));
}
const settle = async () => {
  await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
  await new Promise((r) => setTimeout(r, 30));
};
const notice = () => screen.queryByTestId('pending-restart');
const rowOf = async (name: string) => (await screen.findByText(name)).closest('tr') as HTMLElement;
const rowMarker = (row: HTMLElement) => within(row).queryByTestId('link-pending-restart');

const withConfig = (l: Link, mean_ns: number): Link => ({
  ...l,
  config: { ...l.config, delayer: { ...must(l.config.delayer), mean_ns } },
});

describe('pending restart: the local configuration equals the running one', () => {
  it.each(['/', '/settings'])('shows nothing on %s', async (path) => {
    setup();
    renderApp(path);
    await settle();
    expect(notice()).not.toBeInTheDocument();
    expect(screen.queryByTestId('link-pending-restart')).not.toBeInTheDocument();
  });

  it("ids and the controller's extra keys (assigned id, dir, lcore) do not count", async () => {
    seedStore({ links: [fx.link({ id: 41 }), fx.linkRev({ id: 42 })] });
    api.getEmulation.mockResolvedValue(fx.emulation('running', { config: RUNNING() }));
    renderApp('/settings');
    await settle();
    expect(notice()).not.toBeInTheDocument();
  });

  it('null and absent optional values are the same (seed, histogram, a missing stage)', async () => {
    const local = [fx.link({ config: { ...fx.config(), seed: null, histogram: null, limiter: null } })];
    const running = RUNNING([
      fx.link({
        config: dropNulls({ ...fx.config(), seed: null, histogram: null, limiter: null }) as ReturnType<
          typeof fx.config
        >,
      }),
    ]);
    setup({ local, running });
    renderApp('/settings');
    await settle();
    expect(notice()).not.toBeInTheDocument();
  });

  it('the order of the keys does not matter', async () => {
    const running = RUNNING();
    running.links = running.links.map((l) => Object.fromEntries(Object.entries(l).reverse()) as typeof l);
    setup({ running });
    renderApp('/settings');
    await settle();
    expect(notice()).not.toBeInTheDocument();
  });
});

describe('pending restart: something differs', () => {
  it.each(['/', '/settings'])('a changed link config shows the notice on %s (text mentions restart)', async (path) => {
    setup({ local: [withConfig(fx.link(), 2000), fx.linkRev()] });
    renderApp(path);
    expect(await screen.findByTestId('pending-restart')).toHaveTextContent(/restart/i);
  });

  it('only the changed link is marked in the list; the unchanged one is not', async () => {
    setup({ local: [withConfig(fx.link(), 2000), fx.linkRev()] });
    renderApp('/settings');
    await screen.findByTestId('pending-restart');
    expect(rowMarker(await rowOf('a-b'))).toBeInTheDocument();
    expect(rowMarker(await rowOf('b-a'))).not.toBeInTheDocument();
    expect(screen.getAllByTestId('link-pending-restart')).toHaveLength(1);
  });

  it.each([
    ['ingress', { ingress: 'net_null7' }],
    ['egress', { egress: 'net_null7' }],
    ['reordering', { config: { ...fx.config(), reordering: true } }],
    ['the loss model', { config: { ...fx.config(), loss_model: { type: 'Random' as const, p: 0.5 } } }],
    ['a stage that was added', { config: { ...fx.config(), limiter: { mode: 'Spacer' as const, rate_bps: 1e9 } } }],
    ['a stage that was removed', { config: { ...fx.config(), delayer: null } }],
    ['the buffer capacity', { config: { ...fx.config(), buffer_capacity: 999 } }],
    ['the seed', { config: { ...fx.config(), seed: 5 } }],
  ])('a changed %s of one link is a difference, marked on that row only', async (_what, patch) => {
    setup({ local: [fx.link(patch as Partial<Link>), fx.linkRev()] });
    renderApp('/settings');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(rowMarker(await rowOf('a-b'))).toBeInTheDocument();
    expect(rowMarker(await rowOf('b-a'))).not.toBeInTheDocument();
  });

  it('a link that only exists locally is marked as pending and raises the notice', async () => {
    setup({ local: [...LINKS(), fx.loopLink({ id: 7 })] });
    renderApp('/settings');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(rowMarker(await rowOf('loop'))).toBeInTheDocument();
    expect(rowMarker(await rowOf('a-b'))).not.toBeInTheDocument();
  });

  it('a link that was removed locally raises the notice (nothing to mark)', async () => {
    setup({ local: [fx.link()] });
    renderApp('/settings');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(screen.queryByTestId('link-pending-restart')).not.toBeInTheDocument();
  });

  it('a renamed link counts as new locally and missing in the running one', async () => {
    setup({ local: [fx.link({ name: 'renamed' }), fx.linkRev()] });
    renderApp('/settings');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(rowMarker(await rowOf('renamed'))).toBeInTheDocument();
  });

  it('links are matched by name, not by their position or id', async () => {
    setup({ local: [fx.linkRev({ id: 1 }), fx.link({ id: 3 })] }); // swapped order and ids, same content
    renderApp('/settings');
    await settle();
    expect(notice()).not.toBeInTheDocument();
  });

  it('the notice is shown while starting too', async () => {
    setup({ local: [withConfig(fx.link(), 2000), fx.linkRev()], state: 'starting' });
    renderApp('/settings');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
  });

  it.each(['stopped', 'failed'] as EmulationState[])(
    'nothing is compared while %s (no running configuration)',
    async (state) => {
      setup({ local: [withConfig(fx.link(), 2000), fx.linkRev()], running: null, state });
      renderApp('/settings');
      await settle();
      expect(notice()).not.toBeInTheDocument();
      expect(screen.queryByTestId('link-pending-restart')).not.toBeInTheDocument();
    },
  );

  it('nothing is flagged while stopping, when the run is going away anyway', async () => {
    setup({ local: [withConfig(fx.link(), 2000), fx.linkRev()], state: 'stopping' });
    renderApp('/settings');
    await settle();
    expect(notice()).not.toBeInTheDocument();
  });

  it('with no local links but a running emulation the notice is not the way to tell: the load button is (see below)', async () => {
    setup({ local: [] });
    renderApp('/settings');
    expect(await screen.findByRole('button', { name: 'Load running configuration' })).toBeInTheDocument();
  });

  it('a corrupt local document does not break the page (no comparison, the reset alert shows)', async () => {
    seedRaw('{not json');
    api.getEmulation.mockResolvedValue(fx.emulation('running', { config: RUNNING() }));
    renderApp('/settings');
    expect(await screen.findByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(notice()).not.toBeInTheDocument();
  });
});

describe('Load running configuration', () => {
  const button = () => screen.queryByRole('button', { name: 'Load running configuration' });
  const running = () => RUNNING([fx.link(), fx.linkRev()]);

  it.each(['/', '/settings'])('is offered on %s when an emulation runs and the browser has no links', async (path) => {
    setup({ local: [], running: running() });
    renderApp(path);
    expect(await screen.findByRole('button', { name: 'Load running configuration' })).toBeEnabled();
  });

  it('is also offered while the emulation is starting', async () => {
    setup({ local: [], running: running(), state: 'starting' });
    renderApp('/');
    expect(await screen.findByRole('button', { name: 'Load running configuration' })).toBeInTheDocument();
  });

  it('is also offered when nothing is stored at all', async () => {
    localStorage.clear();
    api.getEmulation.mockResolvedValue(fx.emulation('running', { config: running() }));
    renderApp('/');
    expect(await screen.findByRole('button', { name: 'Load running configuration' })).toBeInTheDocument();
    expect(rawStore()).toBeNull(); // offering writes nothing
  });

  it.each(['/', '/settings'])('is not offered on %s when the browser has links', async (path) => {
    setup({ local: [fx.link()], running: running() });
    renderApp(path);
    await settle();
    expect(button()).not.toBeInTheDocument();
  });

  it('is not offered while nothing runs', async () => {
    setup({ local: [], running: null, state: 'stopped' });
    renderApp('/');
    await settle();
    expect(button()).not.toBeInTheDocument();
  });

  it('loads the running links (ids 1..n, running order) into the local document', async () => {
    seedStore({ links: [] });
    api.getEmulation.mockResolvedValue(fx.emulation('running', { config: running() }));
    const user = userEvent.setup();
    renderApp('/');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(storedDoc().links).toHaveLength(2));
    const doc = storedDoc();
    expect(doc.version).toBe(1);
    expect(doc.links.map((l) => [l.id, l.name, l.ingress, l.egress])).toEqual([
      [1, 'a-b', '0000:65:00.0', '0000:65:00.1'],
      [2, 'b-a', '0000:65:00.1', '0000:65:00.0'],
    ]);
    expect(dropNulls(doc.links[0].config)).toEqual(dropNulls(fx.config()));
    expect(dropNulls(doc.links[1].config)).toEqual(dropNulls(fx.config2()));
    expect(Object.keys(doc.links[0]).sort()).toEqual(
      ['config', 'egress', 'id', 'ingress', 'name'].filter((k) => k in doc.links[0]).sort(),
    );
    expect(Object.keys(doc).sort()).toEqual(['links', 'version']);
  });

  it('afterwards the button is gone and there is no pending restart: the document equals the running one', async () => {
    setup({ local: [], running: running() });
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    expect(await screen.findByText('a-b')).toBeInTheDocument();
    expect(screen.getByText('b-a')).toBeInTheDocument();
    expect(button()).not.toBeInTheDocument();
    await new Promise((r) => setTimeout(r, 30));
    expect(notice()).not.toBeInTheDocument();
    expect(screen.queryByTestId('link-pending-restart')).not.toBeInTheDocument();
  });

  it('on the Dashboard the loaded document is what the next Start would send', async () => {
    setup({ local: [], running: running(), state: 'running' });
    const user = userEvent.setup();
    renderApp('/');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(storedDoc().links).toHaveLength(2));
    await waitFor(() => expect(button()).not.toBeInTheDocument());
    expect(notice()).not.toBeInTheDocument();
  });

  it('loading does not request anything beyond the emulation state', async () => {
    setup({ local: [], running: running() });
    const user = userEvent.setup();
    renderApp('/settings');
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await screen.findByText('a-b');
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(api.stopEmulation).not.toHaveBeenCalled();
  });
});
