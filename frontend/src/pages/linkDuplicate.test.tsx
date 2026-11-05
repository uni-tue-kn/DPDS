import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import { MemoryRouter, useLocation, useNavigate } from 'react-router';
import type { Location, NavigateFunction } from 'react-router';
import App from '../App';
import type { Link } from '../api/types';
import * as fx from '../test/fixtures';
import {
  accuracyOf,
  rawStore,
  renderApp,
  sameConfig,
  seedLinks,
  storedDoc,
  storedLinks,
  typeInto,
  watchStoreWrites,
} from '../test/helpers';

// Spec 27 (user 2026-11-04): a button "Duplicate <link name>" (icon bi-files, P4TG's "Clone") sits in each Links-list row
// between Edit and Delete. It opens the NEW-link editor pre-filled with a copy: same config and frame size, and
// the name `<name>_copy` (taken: `<name>_copy2`, `_copy3` ...; the base is shortened so the whole name has at most 32
// characters). Ingress and Egress are left UNSET (both are required, so the user must pick ports before saving; a copy on the
// source's ports would collide with it at start). Spec 41: there is no worker field any more. The seed and the frame size are still copied. Nothing is stored until the
// user saves; saving goes through the normal checks. Item 6: the router state of Duplicate is consumed once (see the last block).
// Tester's choices where the spec is silent: the shortened base is `name.slice(0, 32 - suffix.length)`; the names that count as
// taken are those of ALL stored links (exact match); the URL of the editor is not pinned (only its heading "New link").

const framed = (l: Link, frame: number | null): Link => ({
  ...l,
  avg_frame_size_byte: frame,
});
const ab = () => framed(fx.link(), 512); // a-b, id 1, config(), frame size 512
const ba = () => fx.linkRev(); // b-a, id 3, config2(), no frame size

const rowOf = async (name: string) => (await screen.findByText(name)).closest('tr') as HTMLElement;
const dupButton = (name: string) => screen.getByRole('button', { name: `Duplicate ${name}` });
const nameField = () => screen.getByLabelText('Name') as HTMLInputElement;
const save = (user: UserEvent) => user.click(screen.getByRole('button', { name: /^save$/i }));
/** The free ports of the fixtures next to a-b (.0 -> .1): the reverse direction. */
const pickPorts = async (user: UserEvent, ingress = '0000:65:00.1', egress = '0000:65:00.0') => {
  await user.selectOptions(screen.getByLabelText('Ingress'), ingress);
  await user.selectOptions(screen.getByLabelText('Egress'), egress);
};
/** a-b (id 1) and a link on other ports (id 3): a-b's ports are taken, the reverse direction is free. */
const abAndLoop = () => [ab(), fx.loopLink({ id: 3 })];

async function duplicate(user: UserEvent, name: string) {
  await user.click(await screen.findByRole('button', { name: `Duplicate ${name}` }));
  await screen.findByRole('heading', { level: 1, name: 'New link' });
  await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
}

async function listPage(links: Link[] = [ab(), ba()]) {
  seedLinks(links);
  const user = userEvent.setup();
  renderApp('/settings');
  await screen.findByText(links[0].name);
  return user;
}

describe('Links list: the Duplicate button', () => {
  it('every row has a button "Duplicate <name>"', async () => {
    await listPage();
    expect(dupButton('a-b')).toBeInTheDocument();
    expect(dupButton('b-a')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /^Duplicate / })).toHaveLength(2);
  });

  it('carries the icon bi-files and no visible text of its own', async () => {
    await listPage();
    expect(dupButton('a-b').querySelector('i.bi.bi-files')).not.toBeNull();
    expect(dupButton('a-b').textContent?.trim()).toBe('');
  });

  it('sits between Edit and Delete in the actions cell of its row', async () => {
    await listPage();
    const cell = (await rowOf('a-b')).querySelector('td:last-child') as HTMLElement;
    const names = Array.from(cell.querySelectorAll('button, a')).map((e) => e.getAttribute('aria-label'));
    expect(names).toEqual(['Inspect a-b', 'Edit a-b', 'Duplicate a-b', 'Delete a-b']);
  });

  it('the column set is unchanged (still seven headers, the last one the hidden "Actions")', async () => {
    await listPage();
    const headers = screen.getAllByRole('columnheader').map((h) => h.textContent?.trim());
    expect(headers).toEqual(['Name', 'Interfaces', 'Delay', 'Reordering', 'Limiter', 'Loss', 'Actions']);
  });

  it('clicking it opens the new-link editor and changes nothing in the store', async () => {
    const user = await listPage();
    const before = rawStore();
    const writes = watchStoreWrites();
    await duplicate(user, 'a-b');
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
    expect(screen.queryByRole('heading', { level: 1, name: 'Edit link' })).not.toBeInTheDocument();
  });
});

describe('Duplicate: the editor opens pre-filled with the copy', () => {
  it('name <name>_copy, same frame size, and NO ports (Ingress and Egress are unset); there is no Worker lcore field', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    expect(nameField()).toHaveValue('a-b_copy');
    expect(screen.getByLabelText('Ingress')).toHaveValue('');
    expect(screen.getByLabelText('Egress')).toHaveValue('');
    expect(screen.queryByLabelText('Worker lcore')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Average frame size')).toHaveValue(512);
  });

  it('the seed is still copied (a link with seed 99)', async () => {
    const seeded = framed(fx.link({ config: fx.config({ seed: 99 }) }), 512);
    const user = await listPage([seeded]);
    await duplicate(user, 'a-b');
    expect(screen.getByLabelText('Seed')).toHaveValue(99);
    expect(screen.getByLabelText('Average frame size')).toHaveValue(512);
  });

  it('shows the same config values as the source (a config that differs from every default)', async () => {
    const user = await listPage();
    await duplicate(user, 'b-a'); // config2: Gamma, correlation on, TBF 40 Gbit/s, Gilbert-Elliot loss, histogram, seed
    expect(nameField()).toHaveValue('b-a_copy');
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(5);
    expect(screen.getByLabelText('Buffer capacity unit')).toHaveDisplayValue('kpackets');
    expect(screen.getByLabelText('Seed')).toHaveValue(42);
    expect(screen.getByLabelText('Delay distribution')).toHaveValue('GammaDistribution');
    expect(accuracyOf()).toBe('Correlation');
    expect(screen.getByLabelText('Limiter mode')).toHaveValue('TokenBucketFilter');
    expect(screen.getByLabelText('Loss model')).toHaveValue('GilbertElliot');
    expect(screen.getByLabelText('Histogram bins')).toHaveValue(64);
    expect(screen.getByLabelText('Delay statistics')).not.toBeChecked();
    // b-a has no frame size
    expect(screen.getByLabelText('Average frame size')).toHaveValue(null);
    expect(screen.getByLabelText('Ingress')).toHaveValue(''); // b-a's ports (.1 -> .0) are not copied either
    expect(screen.getByLabelText('Egress')).toHaveValue('');
  });

  // spec 53 A: the IAT histogram is part of the config, so a copy carries it like `histogram`
  it('copies the IAT histogram: the fields show it, and saving the copy stores it next to the delay histogram', async () => {
    const withIat = fx.link({
      id: 3,
      name: 'iat',
      config: {
        ...fx.config2(),
        histogram: { bins: 32, lo_ns: 1000, hi_ns: 50000 },
        iat_histogram: { bins: 64, lo_ns: 2000, hi_ns: 80000 },
      },
    });
    const user = await listPage([withIat]);
    await duplicate(user, 'iat');
    expect(screen.getByLabelText('IAT histogram bins')).toHaveValue(64);
    expect(screen.getByLabelText('IAT histogram lo')).toHaveValue(2);
    expect(screen.getByLabelText('IAT histogram lo unit')).toHaveDisplayValue('\u00b5s');
    expect(screen.getByLabelText('IAT histogram hi')).toHaveValue(80);
    expect(screen.getByLabelText('IAT histogram hi unit')).toHaveDisplayValue('\u00b5s');
    expect(screen.getByLabelText('Histogram bins')).toHaveValue(32);
    await pickPorts(user);
    await save(user);
    const copy = storedLinks().find((l) => l.name === 'iat_copy');
    expect(copy?.config.iat_histogram).toEqual({ bins: 64, lo_ns: 2000, hi_ns: 80000 });
    expect(copy?.config.histogram).toEqual({ bins: 32, lo_ns: 1000, hi_ns: 50000 });
  });

  it('a source without an IAT histogram gives a copy with three empty IAT fields', async () => {
    const user = await listPage();
    await duplicate(user, 'b-a');
    for (const l of ['IAT histogram bins', 'IAT histogram lo', 'IAT histogram hi']) {
      expect(screen.getByLabelText(l)).toHaveValue(null);
    }
  });

  it('a link on a port that is not an interface (a vdev) gives a copy without ports as well', async () => {
    const user = await listPage([fx.loopLink()]);
    await duplicate(user, 'loop');
    expect(nameField()).toHaveValue('loop_copy');
    expect(screen.getByLabelText('Ingress')).toHaveValue('');
    expect(screen.getByLabelText('Egress')).toHaveValue('');
    expect(screen.queryByRole('option', { name: 'net_null0' })).not.toBeInTheDocument();
  });

  it('both port pickers are still required fields (marker, required, aria-required)', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    for (const label of ['Ingress', 'Egress']) {
      const sel = screen.getByLabelText(label);
      expect(sel).toBeRequired();
      expect(sel).toHaveAttribute('aria-required', 'true');
    }
  });

  it('is the same form as Edit shows for the source: the value of every config field matches', async () => {
    const configValues = () =>
      Array.from(
        document.querySelectorAll<HTMLInputElement | HTMLSelectElement>('fieldset input, fieldset select'),
      ).map((e) => (e.type === 'checkbox' ? String((e as HTMLInputElement).checked) : e.value));
    const user = await listPage();
    await duplicate(user, 'b-a');
    const dupValues = configValues();
    expect(dupValues.length).toBeGreaterThan(10);
    cleanup();
    renderApp('/settings/3');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('b-a'));
    await waitFor(() => expect(configValues()).toHaveLength(dupValues.length));
    expect(dupValues).toEqual(configValues());
  });

  it('a link without stages gives a copy without stages (Add delayer / limiter / dropper)', async () => {
    const bare = fx.link({ name: 'bare', config: fx.configBare() });
    const user = await listPage([bare]);
    await duplicate(user, 'bare');
    expect(nameField()).toHaveValue('bare_copy');
    for (const s of ['delayer', 'limiter', 'dropper']) {
      expect(screen.getByRole('button', { name: `Add ${s}` })).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: `Remove ${s}` })).not.toBeInTheDocument();
    }
  });

  it('opening a second duplicate after cancelling the first shows the SECOND link (no stale form)', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    await user.click(screen.getByRole('link', { name: /^cancel$/i }));
    await duplicate(user, 'b-a');
    expect(nameField()).toHaveValue('b-a_copy');
    expect(screen.getByLabelText('Ingress')).toHaveValue('');
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(5);
    expect(screen.getByLabelText('Buffer capacity unit')).toHaveDisplayValue('kpackets');
  });

  it('"New link" after a cancelled duplicate opens an EMPTY form (the copy does not leak)', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    await user.click(screen.getByRole('link', { name: /^cancel$/i }));
    await user.click(await screen.findByRole('link', { name: /new link/i }));
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    expect(nameField()).toHaveValue('');
    expect(screen.getByLabelText('Ingress')).toHaveValue('');
    expect(screen.getByLabelText('Average frame size')).toHaveValue(null);
  });
});

describe('Duplicate: nothing is stored until Save', () => {
  it('Cancel leaves the stored links as they were and returns to the list', async () => {
    const user = await listPage();
    const before = rawStore();
    const writes = watchStoreWrites();
    await duplicate(user, 'a-b');
    await user.click(screen.getByRole('link', { name: /^cancel$/i }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
    expect(screen.queryByText('a-b_copy')).not.toBeInTheDocument();
  });

  it('typing into the copy writes nothing either', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    const writes = watchStoreWrites();
    await user.type(nameField(), '2');
    expect(writes()).toBe(0);
    expect(storedLinks()).toHaveLength(2);
  });

  it('Save without ports is refused: alert naming the ingress port, nothing written', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/ingress/i);
    expect(writes()).toBe(0);
    expect(storedLinks()).toHaveLength(2);
  });

  it('with only the ingress chosen, Save is refused for the egress port', async () => {
    const user = await listPage(abAndLoop());
    await duplicate(user, 'a-b');
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.1');
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/egress/i);
    expect(writes()).toBe(0);
  });

  it('Save stores the copy as a NEW link (id = max + 1), the source and the others stay as they are', async () => {
    const user = await listPage(abAndLoop());
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    const links = storedLinks();
    expect(links.map((l) => [l.id, l.name])).toEqual([
      [1, 'a-b'],
      [3, 'loop'],
      [4, 'a-b_copy'],
    ]);
    expect(links[0]).toEqual(ab());
    expect(links[1]).toEqual(fx.loopLink({ id: 3 }));
    expect(await rowOf('a-b_copy')).toBeInTheDocument();
  });

  it('the stored copy has the same config and frame size as the source, the ports that were chosen and no worker member', async () => {
    const user = await listPage(abAndLoop());
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    const [src, copy] = [storedLinks()[0], storedLinks()[2]];
    expect(copy.ingress).toBe('0000:65:00.1');
    expect(copy.egress).toBe('0000:65:00.0');
    expect([copy.ingress, copy.egress]).not.toEqual([src.ingress, src.egress]);
    expect(copy).not.toHaveProperty('worker');
    expect(copy.avg_frame_size_byte).toBe(512);
    sameConfig(copy.config, src.config);
  });

  it('the stored copy of a link with every stage keeps that config exactly (b-a, config2)', async () => {
    const user = await listPage([ba()]);
    await duplicate(user, 'b-a');
    await pickPorts(user, '0000:65:00.0', '0000:65:00.1');
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    const copy = storedLinks().find((l) => l.name === 'b-a_copy');
    expect(copy).toBeDefined();
    sameConfig(copy?.config ?? fx.configBare(), fx.config2());
    expect(copy).not.toHaveProperty('worker');
    expect(copy?.avg_frame_size_byte ?? null).toBeNull();
  });

  it('the document keeps nothing but version and links (a stored settings member is dropped), and no presets member appears', async () => {
    localStorage.setItem(
      'dpds.config.v1',
      JSON.stringify({
        version: 1,
        links: [ab()],
        settings: { main_lcore: 2, eal_extra_args: [], poll_interval_ms: 250 },
      }),
    );
    const user = userEvent.setup();
    renderApp('/settings');
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'version']);
    expect(storedDoc().links).toHaveLength(2);
  });
});

describe('Duplicate: the name', () => {
  const named = (...names: string[]): Link[] => names.map((name, i) => fx.link({ id: i + 1, name }));

  it('is <name>_copy', async () => {
    const user = await listPage(named('wan'));
    await duplicate(user, 'wan');
    expect(nameField()).toHaveValue('wan_copy');
  });

  it('is <name>_copy2 when <name>_copy exists', async () => {
    const user = await listPage(named('wan', 'wan_copy'));
    await duplicate(user, 'wan');
    expect(nameField()).toHaveValue('wan_copy2');
  });

  it('is <name>_copy3 when _copy and _copy2 exist', async () => {
    const user = await listPage(named('wan', 'wan_copy', 'wan_copy2'));
    await duplicate(user, 'wan');
    expect(nameField()).toHaveValue('wan_copy3');
  });

  it('takes the first free number even when a later one exists (gaps are filled)', async () => {
    const user = await listPage(named('wan', 'wan_copy', 'wan_copy3'));
    await duplicate(user, 'wan');
    expect(nameField()).toHaveValue('wan_copy2');
  });

  it('counts on past 9', async () => {
    const taken = ['wan', 'wan_copy', ...Array.from({ length: 10 }, (_, i) => `wan_copy${i + 2}`)]; // _copy .. _copy11
    const user = await listPage(named(...taken));
    await duplicate(user, 'wan');
    expect(nameField()).toHaveValue('wan_copy12');
  });

  it('duplicating a copy appends again: <name>_copy_copy', async () => {
    const user = await listPage(named('wan', 'wan_copy'));
    await duplicate(user, 'wan_copy');
    expect(nameField()).toHaveValue('wan_copy_copy');
  });

  it('is judged against ALL stored names, not only those of the same base', async () => {
    const user = await listPage(named('a', 'b', 'a_copy'));
    await duplicate(user, 'b');
    expect(nameField()).toHaveValue('b_copy');
  });

  it('a name of 32 characters is shortened so that base + _copy has 32 characters', async () => {
    const long = 'x'.repeat(32);
    const user = await listPage(named(long));
    await duplicate(user, long);
    expect(nameField()).toHaveValue(`${'x'.repeat(27)}_copy`);
    expect(nameField().value).toHaveLength(32);
  });

  it('with the shortened name taken, the next one is shortened further (_copy2 is one character longer)', async () => {
    const long = 'x'.repeat(32);
    const user = await listPage(named(long, `${'x'.repeat(27)}_copy`));
    await duplicate(user, long);
    expect(nameField()).toHaveValue(`${'x'.repeat(26)}_copy2`);
    expect(nameField().value).toHaveLength(32);
  });

  it('a 28-character name is shortened by exactly the overflow', async () => {
    const name = 'a'.repeat(28);
    const user = await listPage(named(name));
    await duplicate(user, name);
    expect(nameField()).toHaveValue(`${'a'.repeat(27)}_copy`);
  });

  it('a 27-character name needs no shortening (27 + 5 = 32)', async () => {
    const name = 'a'.repeat(27);
    const user = await listPage(named(name));
    await duplicate(user, name);
    expect(nameField()).toHaveValue(`${name}_copy`);
  });

  it('whatever the source, the proposed name satisfies the link-name rule and is not taken', async () => {
    const names = ['A-Z_0.9', 'x'.repeat(32), `${'y'.repeat(26)}.-`, 'wan'];
    for (const source of names) {
      cleanup();
      localStorage.clear();
      const links = named(source, `${source.slice(0, 27)}_copy`);
      const user = await listPage(links);
      await duplicate(user, source);
      const proposed = nameField().value;
      expect(proposed).toMatch(/^[A-Za-z0-9_.-]{1,32}$/);
      expect(links.map((l) => l.name)).not.toContain(proposed);
      expect(proposed).toMatch(/_copy\d*$/);
    }
  });
});

describe('Duplicate: saving goes through the normal checks', () => {
  it('a name that is taken is refused ("already exists"), nothing is written', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    await user.clear(nameField());
    await user.type(nameField(), 'b-a');
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/already exists/i);
    expect(writes()).toBe(0);
    expect(storedLinks()).toHaveLength(2);
  });

  it('an emptied name is refused', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    await user.clear(nameField());
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expect(writes()).toBe(0);
  });

  it('a name that breaks the link-name rule is refused', async () => {
    const user = await listPage();
    await duplicate(user, 'a-b');
    await user.clear(nameField());
    await user.type(nameField(), 'not valid!');
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/name/i);
    expect(writes()).toBe(0);
  });

  it('a required field that was cleared is refused (the copy is a form like any other)', async () => {
    const user = await listPage(abAndLoop());
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await user.clear(screen.getByLabelText('Initialization rate'));
    const writes = watchStoreWrites();
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(writes()).toBe(0);
  });

  it('the copy can be edited before it is saved: the stored link holds the edited values', async () => {
    const user = await listPage(abAndLoop());
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await user.clear(nameField());
    await user.type(nameField(), 'edited');
    await typeInto(user, document.body, 'Buffer capacity', '777', 'packets');
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    const copy = storedLinks().find((l) => l.name === 'edited');
    expect(copy?.config.buffer_capacity).toBe(777);
    expect(copy?.egress).toBe('0000:65:00.0'); // the port that was chosen
    expect(storedLinks()[0]).toEqual(ab()); // the source is not changed by editing the copy
  });
});

describe('Duplicate: the list afterwards', () => {
  it('shows source and copy side by side with the same Delay, Reordering, Limiter and Loss cells', async () => {
    const user = await listPage([ab()]);
    await duplicate(user, 'a-b');
    await pickPorts(user);
    await save(user);
    const src = await rowOf('a-b');
    const copy = await rowOf('a-b_copy');
    const cells = (tr: HTMLElement) =>
      within(tr)
        .getAllByRole('cell')
        .slice(2, 6) // Delay, Reordering, Limiter, Loss (the Interfaces column differs: other ports)
        .map((c) => c.textContent);
    expect(cells(copy)).toEqual(cells(src));
    expect(within(copy).getByRole('button', { name: 'Duplicate a-b_copy' })).toBeInTheDocument();
  });
});

interface Spy {
  location: Location;
  navigate: NavigateFunction;
}
function RouterSpy({ spy }: { spy: Partial<Spy> }) {
  spy.location = useLocation();
  spy.navigate = useNavigate();
  return null;
}

// Spec 27b item 6: the router state `{ copyOf }` of Duplicate is consumed once. After the form is filled, the history entry is
// replaced (not pushed) without the state, so Back / Forward / reload do not pre-fill it again. The tests watch the router
// from a spy component next to the app (MemoryRouter has no browser buttons: navigate(-1) / navigate(1) are Back and Forward).
describe('Duplicate: the router state is consumed once', () => {
  async function openList(links: Link[] = [ab(), ba()]) {
    seedLinks(links);
    const spy: Partial<Spy> = {};
    render(
      <MemoryRouter initialEntries={['/settings']}>
        <RouterSpy spy={spy} />
        <App />
      </MemoryRouter>,
    );
    await screen.findByText(links[0].name);
    return { user: userEvent.setup(), spy };
  }
  const go = (spy: Partial<Spy>, delta: number) =>
    act(async () => {
      spy.navigate?.(delta);
    });
  const copyOfOf = (spy: Partial<Spy>) => (spy.location?.state as { copyOf?: unknown } | null)?.copyOf;

  it('after the form is filled the location no longer carries the copy', async () => {
    const { user, spy } = await openList();
    await duplicate(user, 'b-a');
    expect(nameField()).toHaveValue('b-a_copy');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    expect(spy.location?.pathname).toBe('/settings/new');
  });

  it('the form keeps its pre-filled values and stays editable after the state is gone', async () => {
    const { user, spy } = await openList();
    await duplicate(user, 'b-a');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    expect(nameField()).toHaveValue('b-a_copy');
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(5);
    expect(screen.getByLabelText('Buffer capacity unit')).toHaveDisplayValue('kpackets');
    expect(screen.getByLabelText('Seed')).toHaveValue(42);
    expect(screen.getByLabelText('Delay distribution')).toHaveValue('GammaDistribution');
    await user.type(nameField(), '2');
    expect(nameField()).toHaveValue('b-a_copy2');
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(5);
    expect(screen.getByLabelText('Buffer capacity unit')).toHaveDisplayValue('kpackets');
  });

  it('the entry is REPLACED, not pushed: Back leads straight to the list', async () => {
    const { user, spy } = await openList();
    await duplicate(user, 'b-a');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    await go(spy, -1);
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeInTheDocument();
    expect(spy.location?.pathname).toBe('/settings');
  });

  it('Forward to the form again shows an EMPTY new-link form, not the copy', async () => {
    const { user, spy } = await openList();
    await duplicate(user, 'b-a');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    await go(spy, -1);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    await go(spy, 1);
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    expect(nameField()).toHaveValue('');
    expect(screen.getByLabelText('Ingress')).toHaveValue('');
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(100); // the defaults of a new form (100 kpackets), not b-a's 5 kpackets
    expect(screen.getByLabelText('Seed')).toHaveValue(null);
  });

  it('a second Duplicate after Back still pre-fills (each click carries its own state)', async () => {
    const { user, spy } = await openList();
    await duplicate(user, 'b-a');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    await go(spy, -1);
    await duplicate(user, 'a-b');
    expect(nameField()).toHaveValue('a-b_copy');
    expect(screen.getByLabelText('Average frame size')).toHaveValue(512);
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
  });

  it('the replace writes nothing to the store', async () => {
    const { user, spy } = await openList();
    const before = rawStore();
    const writes = watchStoreWrites();
    await duplicate(user, 'a-b');
    await waitFor(() => expect(copyOfOf(spy)).toBeUndefined());
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
  });

  it('a plain "New link" never carries a copy', async () => {
    const { user, spy } = await openList();
    await user.click(screen.getByRole('link', { name: /new link/i }));
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    expect(copyOfOf(spy)).toBeUndefined();
    expect(nameField()).toHaveValue('');
  });
});
