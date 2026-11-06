import { act, screen, waitFor, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { vi } from 'vitest';
import type { ConfigDto, Link } from '../api/types';
import { dropNulls } from '../test/dropNulls';
import * as fx from '../test/fixtures';
import {
  addLink,
  addLinkButton,
  addStages,
  clickReset,
  clickSave,
  closeModal,
  confirmModal,
  expectRequired,
  expectStartBody,
  exportButton,
  findToast,
  footerButton,
  histogramDialog,
  importButton,
  infoDialog,
  linkDialog,
  linkRows,
  modalAlert,
  numberField,
  openHistogram,
  openModal,
  openSettings,
  pickUnit,
  portSelect,
  portsLoaded,
  portsOfRows,
  queryToast,
  rawStore,
  removeStages,
  renderApp,
  renderSettings,
  resetButton,
  rowOf,
  STORAGE_KEY,
  saveButton,
  seedLinks,
  seedRaw,
  stageSwitch,
  storedDoc,
  storedLinks,
  typeInto,
  unitSelect,
  watchStoreWrites,
} from '../test/helpers';
import { api } from '../test/mockApi';
import { must } from '../test/must';
import type { HelpKey } from './fieldHelp';
import { HELP } from './fieldHelp';

// The Settings page as flows (spec 63): the draft and Save, Add / Duplicate / Delete, the two modals (Close discards, Confirm commits,
// an invalid Confirm shows an alert in the modal), the fields of the modals, Save validation, Reset, Import / Export, pending restart,
// Load running configuration, the corrupt store and the InfoBox (spec 64: links have no names, rows are told by position and ports). Toasts are checked by their message only. The pure rules (units,
// `formToDto`, import checks, pending-restart comparison) are in linkForm.test.ts and localStore.test.ts.

const MU = 'µs';

/** The page with `links` stored (default: a-b) and the first row on screen. */
async function page(links: Link[] = [fx.link()]) {
  seedLinks(links);
  const user = await renderSettings();
  await waitFor(() => expect(portSelect('RX', 1)).toHaveValue(must(links[0]).ingress));
  await portsLoaded(1);
  return user;
}
/** The `[rx, tx]` pair of each link, as the table shows it in the selects. */
const pairs = (links: Link[]) => links.map((l) => [l.ingress, l.egress]);
const configWith = (delayer: Partial<NonNullable<ConfigDto['delayer']>>): ConfigDto =>
  fx.config({ delayer: { ...must(fx.config().delayer), ...delayer } });
/** A stored a-b whose config the form refuses on Save (reordering with the Spacer limiter): loads fine, fails `formToDto`. */
const brokenConfig = (): ConfigDto => ({
  ...fx.config(),
  reordering: true,
  limiter: { mode: 'Spacer', rate_bps: 1e9 },
});
/** A new row with both ports chosen and the Link settings modal open, the three stages on (a valid initialization rate typed). */
async function newFullLink(): Promise<UserEvent> {
  const user = await renderSettings();
  await addLink(user);
  await portsLoaded(1);
  await user.selectOptions(portSelect('RX', 1), '0000:65:00.0');
  await user.selectOptions(portSelect('TX', 1), '0000:65:00.1');
  await openSettings(user, 1);
  await addStages(user);
  await typeInto(user, document.body, 'Initialization rate', '10', 'Mpps');
  return user;
}
const body = () => document.body;
const optionsOf = (label: string) =>
  within(unitSelect(body(), label))
    .getAllByRole('option')
    .map((o) => o.textContent);

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(URL, 'createObjectURL');
  Reflect.deleteProperty(URL, 'revokeObjectURL');
});

/** `config` with every missing histogram value replaced by its default, as a save writes it. */
function explicitHistograms(config: ConfigDto): ConfigDto {
  return {
    ...config,
    histogram: { bins: 16, lo_ns: 1024, hi_ns: 2048, ...dropNulls(config.histogram ?? {}) },
    iat_histogram: { bins: 16, lo_ns: 0, hi_ns: 1024, ...dropNulls(config.iat_histogram ?? {}) },
  };
}

describe('the draft: nothing is stored before Save', () => {
  it('Add, a port, a modal Confirm, Duplicate and Delete write nothing; Save writes once', async () => {
    const user = await page([fx.link(), fx.loopLink({ id: 2 })]);
    const before = rawStore();
    const writes = watchStoreWrites();
    await addLink(user);
    await user.selectOptions(portSelect('RX', 3), '0000:65:00.1');
    await openSettings(user, 1);
    await typeInto(user, document.body, 'Buffer capacity', '777', 'packets');
    await confirmModal(user);
    await user.click(screen.getByRole('button', { name: 'Duplicate link 2' }));
    await user.click(screen.getByRole('button', { name: 'Delete link 3' })); // the copy, inserted after its source
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
    await clickSave(user); // the new row has no TX port yet: refused, still nothing written
    expect(writes()).toBe(0);
    await user.selectOptions(portSelect('TX', 3), '0000:65:00.0');
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(writes()).toBe(1);
  });

  it('Save writes the whole draft in table order with the ids kept, in one write', async () => {
    const user = await page([fx.link({ id: 3 }), fx.loopLink({ id: 10 })]);
    await user.selectOptions(portSelect('RX', 1), '0000:65:00.1');
    const writes = watchStoreWrites();
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(writes()).toBe(1);
    expect(storedLinks().map((l) => [l.id, l.ingress])).toEqual([
      [3, '0000:65:00.1'],
      [10, 'net_null0'],
    ]);
    const untouched = fx.loopLink({ id: 10 });
    // an untouched row is saved as it was, its histograms with explicit (default) values
    expect(dropNulls(must(storedLinks()[1]))).toEqual(
      dropNulls({ ...untouched, config: explicitHistograms(untouched.config) }),
    );
  });

  it('two new rows get the ids max + 1 and max + 2 at save time (the first id of an empty store is 1)', async () => {
    const user = await renderSettings();
    await addLink(user);
    await addLink(user);
    await portsLoaded(2);
    await user.selectOptions(portSelect('RX', 1), '0000:65:00.0');
    await user.selectOptions(portSelect('TX', 1), '0000:65:00.1');
    await user.selectOptions(portSelect('RX', 2), '0000:65:00.1');
    await user.selectOptions(portSelect('TX', 2), '0000:65:00.0');
    expect(rawStore()).toBeNull();
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(storedLinks().map((l) => [l.id, l.ingress, l.egress])).toEqual([
      [1, '0000:65:00.0', '0000:65:00.1'],
      [2, '0000:65:00.1', '0000:65:00.0'],
    ]);
    for (const l of storedLinks()) expect(l).not.toHaveProperty('name');
  });

  it('Reset puts the draft back to the stored links and says so; it asks nothing and writes nothing', async () => {
    const user = await page([fx.link(), fx.linkRev()]);
    const before = rawStore();
    const writes = watchStoreWrites();
    await user.click(screen.getByRole('button', { name: 'Delete link 2' }));
    await addLink(user);
    expect(portsOfRows()).toEqual([...pairs([fx.link()]), ['', '']]);
    await clickReset(user);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await findToast('Settings reset successfully.');
    expect(portsOfRows()).toEqual(pairs([fx.link(), fx.linkRev()]));
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
  });

  it('leaving the page discards an unsaved draft', async () => {
    const user = await page();
    await addLink(user);
    await user.click(screen.getByRole('link', { name: 'Dashboard' }));
    await screen.findByRole('heading', { level: 1, name: 'Dashboard' });
    await user.click(screen.getByRole('link', { name: 'Settings' }));
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    await waitFor(() => expect(portSelect('RX', 1)).toHaveValue(fx.link().ingress));
    expect(linkRows()).toHaveLength(1);
    expect(storedLinks().map((l) => l.ingress)).toEqual([fx.link().ingress]);
  });

  describe('a change of the store from outside (another tab)', () => {
    const otherTab = (links: Link[]) => {
      seedLinks(links);
      act(() => {
        window.dispatchEvent(new StorageEvent('storage', { key: STORAGE_KEY }));
      });
    };

    it('a dirty draft keeps its unsaved edit; Reset then takes the new stored links', async () => {
      const user = await page();
      await addLink(user);
      otherTab([fx.linkRev()]);
      await new Promise((r) => setTimeout(r, 30));
      expect(portsOfRows()).toEqual([...pairs([fx.link()]), ['', '']]);
      await clickReset(user);
      expect(portsOfRows()).toEqual(pairs([fx.linkRev()]));
    });

    it('a clean draft follows it', async () => {
      await page();
      otherTab([fx.linkRev()]);
      await waitFor(() => expect(portsOfRows()).toEqual(pairs([fx.linkRev()])));
    });
  });

  it('a failing store write is reported and is not a success', async () => {
    const user = await page();
    await openSettings(user, 1);
    await typeInto(user, body(), 'Seed', '5');
    await confirmModal(user);
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota exceeded');
    });
    await clickSave(user);
    await findToast('quota exceeded');
    expect(screen.queryByText('Settings saved successfully.')).not.toBeInTheDocument();
  });
});

describe('creating a link end to end', () => {
  it('Add link, ports, the three stages and both histograms, Save: the stored link is what was typed, and Start sends it', async () => {
    const user = await newFullLink();
    await typeInto(user, body(), 'Buffer capacity', '2000', 'packets');
    await typeInto(user, body(), 'Mean delay', '2.5', MU);
    await typeInto(user, body(), 'Delay std', '500', 'ns');
    await typeInto(user, body(), 'Rate', '5', 'Gbit/s');
    await typeInto(user, body(), 'Random loss probability', '0.01');
    await openHistogram(user, 1);
    await typeInto(user, body(), 'Histogram bins', '64');
    await typeInto(user, body(), 'Histogram lo', '1', 'ms');
    await typeInto(user, body(), 'IAT histogram bins', '32');
    await confirmModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');

    const expected: ConfigDto = {
      buffer_capacity: 2000,
      reordering: false,
      delayer: {
        distribution: 'NormalDistribution',
        mean_ns: 2500,
        std_ns: 500,
        activate_correlation: false,
        half_life_period_ns: 0,
        initialization_rate_pps: 1e7,
      },
      limiter: { mode: 'Spacer', rate_bps: 5e9 },
      loss_model: { type: 'Random', p: 0.01 },
      seed: null,
      histogram: { bins: 64, lo_ns: 1e6, hi_ns: 2048 },
      iat_histogram: { bins: 32, lo_ns: 0, hi_ns: 1024 },
      delay_stats: true,
    };
    const saved = storedLinks();
    expect(saved).toHaveLength(1);
    expect(dropNulls(saved[0])).toEqual(
      dropNulls({ id: 1, ingress: '0000:65:00.0', egress: '0000:65:00.1', config: expected }),
    );

    await user.click(screen.getByRole('link', { name: 'Dashboard' }));
    const start = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(start).toBeEnabled());
    await user.click(start);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    expectStartBody(api.startEmulation.mock.calls[0][0], storedDoc());
  });

  it('a stage switched off in the modal is saved as no stage; the rest of the config stays', async () => {
    const user = await page([fx.link({ id: 3, config: fx.config2() })]);
    await openSettings(user, 1);
    await removeStages(user);
    await confirmModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    const c = must(storedLinks()[0]).config;
    expect([c.delayer ?? null, c.limiter ?? null, c.loss_model ?? null]).toEqual([null, null, null]);
    expect(c.buffer_capacity).toBe(fx.config2().buffer_capacity);
  });
});

describe('Save validates the draft link by link; the first problem blocks it with a message', () => {
  const expectBlocked = async (text: string | RegExp, writes: () => number) => {
    await findToast(text);
    expect(writes()).toBe(0);
  };

  it('a port used as RX port by two links: the second one is named', async () => {
    const user = await page([fx.link(), fx.link({ id: 2, egress: 'net_null0' })]);
    const writes = watchStoreWrites();
    await clickSave(user);
    await expectBlocked(/Link 2: .*RX port of more than one link/i, writes);
  });

  it('a port used as TX port by two links: the second one is named', async () => {
    const user = await page([fx.link(), fx.link({ id: 2, ingress: 'net_null0' })]);
    const writes = watchStoreWrites();
    await clickSave(user);
    await expectBlocked(/Link 2: .*TX port of more than one link/i, writes);
  });

  it('a config that the form refuses (reordering with the Spacer) names its link position and the problem', async () => {
    const user = await page([fx.link({ config: brokenConfig() })]);
    const writes = watchStoreWrites();
    await clickSave(user);
    await expectBlocked(/Link 1: .*(Reordering|Spacer)/i, writes);
  });

  it('the first problem in table order is the one reported, even when later rows have problems too', async () => {
    const loop = (id: number) =>
      fx.loopLink({ id, ingress: `net_null${id}`, egress: `net_null${id}`, config: brokenConfig() });
    const user = await page([fx.link({ id: 1 }), loop(2), loop(3)]);
    const writes = watchStoreWrites();
    await clickSave(user);
    await expectBlocked(/Link 2: /, writes);
    expect(screen.queryByText(/Link 3/)).not.toBeInTheDocument();
  });

  it('a refusal leaves the draft as it is; once the problems are fixed the save goes through with ONE write', async () => {
    const user = await page([fx.link({ config: brokenConfig() })]);
    await addLink(user);
    const writes = watchStoreWrites();
    await clickSave(user);
    await expectBlocked(/Link 1: .*(Reordering|Spacer)/i, writes);
    expect(linkRows()).toHaveLength(2);
    await openSettings(user, 1);
    await user.click(screen.getByRole('radio', { name: 'Basic' }));
    await confirmModal(user);
    await clickSave(user);
    await expectBlocked('Link 2: Select an RX port', writes);
    await user.selectOptions(portSelect('RX', 2), '0000:65:00.1');
    await user.selectOptions(portSelect('TX', 2), '0000:65:00.0');
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(writes()).toBe(1);
  });

  it.each([
    ['an RX port', false, 'Link 1: Select an RX port'],
    ['a TX port', true, 'Link 1: Select a TX port'],
  ])('a new row without ports is refused: it needs %s', async (_what, rxChosen, message) => {
    const user = await renderSettings();
    await addLink(user);
    await portsLoaded(1);
    if (rxChosen) await user.selectOptions(portSelect('RX', 1), '0000:65:00.0');
    await clickSave(user);
    await findToast(message);
  });
});

describe('the table: rows, ports, Add link, Duplicate, Delete', () => {
  it('has the columns RX port, TX port, Histogram, Settings and Options and one row per link in stored order, controls named by position', async () => {
    await page([fx.linkRev(), fx.link()]);
    const heads = screen.getAllByRole('columnheader').map((h) => h.textContent?.trim());
    expect(heads).toHaveLength(5);
    for (const head of ['RX port', 'TX port', 'Histogram', 'Settings', 'Options']) expect(heads).toContain(head);
    expect(portsOfRows()).toEqual(pairs([fx.linkRev(), fx.link()]));
    expect(within(rowOf(1)).queryAllByRole('textbox')).toHaveLength(0); // no name field
    for (const name of ['Histogram of link 2', 'Settings of link 2', 'Duplicate link 2', 'Delete link 2']) {
      expect(within(rowOf(2)).getByRole('button', { name })).toBeInTheDocument();
    }
  });

  it('ports show by netdev; unusable and uplink ports are disabled; a stored port that is no interface stays selected', async () => {
    await page([fx.link(), fx.loopLink({ id: 2 })]);
    expect(portSelect('RX', 1)).toHaveDisplayValue(/^enp101s0f0/);
    expect(portSelect('TX', 1)).toHaveDisplayValue(/^enp101s0f1/);
    expect(portSelect('RX', 2)).toHaveValue('net_null0');
    for (const netdev of [/enp179s0f0/, /eno1/]) {
      expect(within(portSelect('RX', 1)).getByRole('option', { name: netdev })).toBeDisabled();
    }
  });

  it('a port another row uses is disabled in that role, with a reason (draft rows count)', async () => {
    const user = await page([fx.link(), fx.loopLink({ id: 2, ingress: 'net_null1', egress: 'net_null1' })]);
    await user.selectOptions(portSelect('RX', 2), '0000:65:00.1');
    const used = within(portSelect('RX', 1)).getByRole('option', { name: /enp101s0f1/ });
    expect(used).toBeDisabled();
    expect(used).toHaveTextContent(/already used as RX port/);
    // only in its own role: the same port stays selectable as TX port of row 1 (nobody else sends out of it)
    expect(within(portSelect('TX', 1)).getByRole('option', { name: /enp101s0f1/ })).toBeEnabled();
  });

  it('Add link appends an empty row with no ports, and opens nothing', async () => {
    const user = await page([fx.link(), fx.loopLink({ id: 2, ingress: 'net_null1', egress: 'net_null1' })]);
    await addLink(user);
    await addLink(user);
    expect(portsOfRows().slice(2)).toEqual([
      ['', ''],
      ['', ''],
    ]);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('in an empty table the first row is empty with the default config: no stage on, 100000 packets of buffer', async () => {
    const user = await renderSettings();
    expect(screen.getByText('No links configured yet.')).toBeInTheDocument();
    await addLink(user);
    expect(portsOfRows()).toEqual([['', '']]);
    expect(screen.queryByText('No links configured yet.')).not.toBeInTheDocument();
    await openSettings(user, 1);
    for (const s of ['delayer', 'limiter', 'dropper'] as const) expect(stageSwitch(s)).not.toBeChecked();
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(100);
    expect(screen.getByLabelText('Buffer capacity unit')).toHaveDisplayValue('kpackets');
  });

  it('Duplicate inserts the copy after the row without ports and says "Link duplicated."; the copy saves with the source config', async () => {
    const source = fx.linkRev({ config: fx.config2(), avg_frame_size_byte: 1518 });
    const user = await page([source]);
    await user.click(screen.getByRole('button', { name: 'Duplicate link 1' }));
    expect(portsOfRows()).toEqual([...pairs([source]), ['', '']]);
    await findToast('Link duplicated.');
    await user.selectOptions(portSelect('RX', 2), '0000:65:00.0');
    await user.selectOptions(portSelect('TX', 2), '0000:65:00.1');
    await clickSave(user);
    await findToast('Settings saved successfully.');
    const [kept, copy] = storedLinks();
    expect([kept.id, copy.id]).toEqual([3, 4]);
    expect(dropNulls(copy.config)).toEqual(dropNulls(explicitHistograms(source.config)));
    expect(copy.avg_frame_size_byte).toBe(1518);
  });

  it('Delete removes the row without a confirmation or a write; the rows renumber and the ids stay', async () => {
    const user = await page([fx.link(), fx.linkRev()]);
    const writes = watchStoreWrites();
    await user.click(screen.getByRole('button', { name: 'Delete link 1' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(portsOfRows()).toEqual(pairs([fx.linkRev()]));
    expect(writes()).toBe(0);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(storedLinks().map((l) => [l.id, l.ingress])).toEqual([[3, fx.linkRev().ingress]]);
    await user.click(screen.getByRole('button', { name: 'Delete link 1' }));
    await clickSave(user);
    await waitFor(() => expect(storedLinks()).toEqual([]));
    expect(screen.getByText('No links configured yet.')).toBeInTheDocument();
  });
});

describe('the Link settings modal: Close discards, Confirm commits', () => {
  it('is titled with the ports of its row (netdev labels), the histogram modal too; a row without ports has the plain titles', async () => {
    const user = await page([fx.link(), fx.linkRev()]);
    const title = (d: HTMLElement) => d.querySelector('.modal-title')?.textContent?.trim();
    expect(title(await openSettings(user, 2))).toBe('Link enp101s0f1 → enp101s0f0');
    await closeModal(user);
    expect(title(await openHistogram(user, 2))).toBe('Histograms of link enp101s0f1 → enp101s0f0');
    await closeModal(user);
    await addLink(user);
    expect(title(await openSettings(user, 3))).toBe('Link settings');
    await closeModal(user);
    expect(title(await openHistogram(user, 3))).toBe('Histograms');
  });

  it('Close, the header X and Escape discard the edits; every opening starts from the draft row', async () => {
    const user = await page();
    const attempts: [string, () => Promise<void>][] = [
      ['footer Close', () => closeModal(user)],
      ['header X', async () => user.click(within(must(linkDialog())).getByLabelText('Close'))],
      ['Escape', async () => user.keyboard('{Escape}')],
    ];
    for (const [how, close] of attempts) {
      await openSettings(user, 1);
      await typeInto(user, body(), 'Seed', '99');
      await close();
      expect(linkDialog(), how).toBeNull();
      await openSettings(user, 1);
      expect(screen.getByLabelText('Seed'), how).toHaveValue(null);
      await closeModal(user);
    }
  });

  it('Confirm (the button, or Enter in a field) writes the edits into the draft row and closes; nothing is stored until Save', async () => {
    const user = await page();
    const writes = watchStoreWrites();
    await openSettings(user, 1);
    await typeInto(user, body(), 'Buffer capacity', '777', 'packets');
    await confirmModal(user);
    expect(linkDialog()).toBeNull();
    await openSettings(user, 1);
    await typeInto(user, body(), 'Seed', '5');
    await user.type(screen.getByLabelText('Seed'), '{Enter}');
    expect(linkDialog()).toBeNull();
    expect(writes()).toBe(0);
    await openSettings(user, 1);
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(777);
    expect(screen.getByLabelText('Seed')).toHaveValue(5);
    await closeModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(must(storedLinks()[0]).config).toMatchObject({ buffer_capacity: 777, seed: 5 });
  });

  it('an invalid Confirm shows an alert in the modal with the first problem; the modal stays and the draft is unchanged', async () => {
    const user = await page();
    await openSettings(user, 1);
    await user.clear(screen.getByLabelText('Buffer capacity'));
    await user.clear(screen.getByLabelText('Mean delay'));
    await confirmModal(user);
    const alert = must(modalAlert());
    expect(alert).toHaveTextContent(/Buffer capacity/); // the first problem, in section order
    expect(alert).not.toHaveTextContent(/Mean delay/);
    expect(linkDialog()).not.toBeNull();
    await closeModal(user);
    await openSettings(user, 1);
    expect(screen.getByLabelText('Buffer capacity')).toHaveValue(1); // 1000 packets, as stored
  });

  it('the stages are switches: off removes the stage fields, the typed values come back when it is switched on again', async () => {
    const user = await page([fx.link({ config: fx.configBare() })]);
    await openSettings(user, 1);
    expect(screen.queryByLabelText('Mean delay')).not.toBeInTheDocument();
    await user.click(stageSwitch('delayer'));
    await typeInto(user, body(), 'Mean delay', '7', 'ms');
    await user.click(stageSwitch('delayer'));
    expect(screen.queryByLabelText('Mean delay')).not.toBeInTheDocument();
    await user.click(stageSwitch('delayer'));
    expect(screen.getByLabelText('Mean delay')).toHaveValue(7);
    expect(screen.getByLabelText('Mean delay unit')).toHaveDisplayValue('ms');
  });
});

describe('the fields of the Link settings modal', () => {
  it('a unit change converts the shown number exactly and does not edit the value', async () => {
    const user = await page([fx.link({ config: configWith({ mean_ns: 2500 }) })]);
    await openSettings(user, 1);
    expect(screen.getByLabelText('Mean delay')).toHaveValue(2.5);
    expect(screen.getByLabelText('Mean delay unit')).toHaveDisplayValue(MU);
    await pickUnit(user, body(), 'Mean delay', 'ms');
    expect(screen.getByLabelText('Mean delay')).toHaveValue(0.0025);
    await confirmModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(must(must(storedLinks()[0]).config.delayer).mean_ns).toBe(2500);
  });

  it('the rate fields offer the other unit family only with a valid frame size, and convert with it', async () => {
    const user = await newFullLink();
    expect(optionsOf('Initialization rate')).toEqual(['pps', 'kpps', 'Mpps']);
    expect(optionsOf('Rate')).toEqual(['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s']);
    await typeInto(user, body(), 'Average frame size', '63');
    expect(optionsOf('Rate')).toEqual(['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s']);
    await typeInto(user, body(), 'Average frame size', '1518');
    expect(optionsOf('Initialization rate')).toEqual(['pps', 'kpps', 'Mpps', 'bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s']);
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expect(screen.getByLabelText('Initialization rate')).toHaveValue(8.127);
    await confirmModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    const link = must(storedLinks()[0]);
    expect(link.avg_frame_size_byte).toBe(1518);
    expect(must(link.config.delayer).initialization_rate_pps).toBe(8127438);
  });

  it('a rate in the other family without a frame size is refused, naming the frame size', async () => {
    const user = await page([
      fx.link({ avg_frame_size_byte: 1518, config: configWith({ initialization_rate_pps: 8127438 }) }),
    ]);
    await openSettings(user, 1);
    await pickUnit(user, body(), 'Initialization rate', 'Gbit/s');
    expectRequired(screen.getByLabelText('Average frame size'));
    await user.clear(screen.getByLabelText('Average frame size'));
    await confirmModal(user);
    expect(must(modalAlert())).toHaveTextContent(/Average frame size/);
  });

  it('Delay accuracy: Reordering switches the limiter to the token bucket and disables the Spacer; the half-life is only for Correlation', async () => {
    const user = await newFullLink();
    const mode = screen.getByLabelText('Limiter mode') as HTMLSelectElement;
    expect(mode).toHaveValue('Spacer');
    expect(screen.getByLabelText('Half-life period')).toBeDisabled();
    await user.click(screen.getByRole('radio', { name: 'Reordering' }));
    expect(mode).toHaveValue('TokenBucketFilter');
    expect(within(mode).getByRole('option', { name: 'Spacer' })).toBeDisabled();
    expect(screen.getByLabelText('Half-life period')).toBeDisabled();
    await user.click(screen.getByRole('radio', { name: 'Basic' }));
    expect(within(mode).getByRole('option', { name: 'Spacer' })).toBeEnabled();
    await user.click(screen.getByRole('radio', { name: 'Correlation' }));
    expect(screen.getByLabelText('Half-life period')).toBeEnabled();
  });
});

describe('the Histogram modal: Close discards, Confirm commits', () => {
  it('Confirm writes the histogram into the draft row; nothing is stored until Save', async () => {
    const user = await page();
    const writes = watchStoreWrites();
    await openHistogram(user, 1);
    await user.clear(screen.getByLabelText('Histogram bins'));
    await user.type(screen.getByLabelText('Histogram bins'), '64');
    await user.click(screen.getByLabelText('Delay statistics'));
    await confirmModal(user);
    expect(histogramDialog()).toBeNull();
    expect(writes()).toBe(0);
    await openHistogram(user, 1);
    expect(screen.getByLabelText('Histogram bins')).toHaveValue(64);
    expect(screen.getByLabelText('Delay statistics')).not.toBeChecked();
    await closeModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(must(storedLinks()[0]).config.histogram?.bins).toBe(64);
    expect(must(storedLinks()[0]).config.delay_stats).toBe(false);
  });

  it('an invalid Confirm shows an alert naming the histogram field; the modal stays', async () => {
    const user = await page();
    await openHistogram(user, 1);
    await user.type(screen.getByLabelText('IAT histogram bins'), '1.5');
    await confirmModal(user);
    expect(must(modalAlert())).toHaveTextContent(/IAT histogram bins/);
    expect(histogramDialog()).not.toBeNull();
  });

  it('Confirm checks only the histogram fields: a config problem elsewhere does not block it, the Link settings Confirm does check it', async () => {
    const user = await page([fx.link({ config: brokenConfig() })]);
    await openHistogram(user, 1);
    await user.type(screen.getByLabelText('Histogram bins'), '64');
    await confirmModal(user);
    expect(modalAlert()).toBeNull();
    expect(histogramDialog()).toBeNull();
    await openSettings(user, 1);
    await confirmModal(user);
    expect(must(modalAlert())).toHaveTextContent(/Reordering|Spacer/);
  });

  it('"Use suggestion" sets lo to 0 and hi to the suggested value in its own unit, and is what gets saved', async () => {
    const user = await page();
    const dialog = await openHistogram(user, 1);
    await typeInto(user, dialog, 'Histogram hi', '3', MU);
    await user.click(within(dialog).getByRole('button', { name: 'Use suggestion' }));
    expect(within(dialog).getByLabelText('Histogram lo')).toHaveValue(0);
    expect(within(dialog).getByLabelText('Histogram hi')).toHaveValue(4.096);
    expect(within(dialog).getByLabelText('Histogram hi unit')).toHaveDisplayValue(MU);
    expect(within(dialog).queryByRole('button', { name: 'Use suggestion' })).not.toBeInTheDocument();
    await confirmModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    expect(must(storedLinks()[0]).config.histogram).toEqual({ bins: 16, lo_ns: 0, hi_ns: 4096 });
  });

  it('the fields come prefilled with the defaults; an emptied field is the default again, also when saved', async () => {
    const user = await page();
    const dialog = await openHistogram(user, 1);
    expect(numberField(dialog, 'Histogram bins')).toHaveValue(16);
    expect(numberField(dialog, 'IAT histogram bins')).toHaveValue(16);
    expect(numberField(dialog, 'Histogram lo')).toHaveValue(1.024);
    expect(numberField(dialog, 'Histogram hi')).toHaveValue(2.048);
    expect(unitSelect(dialog, 'Histogram hi')).toHaveDisplayValue(MU);
    expect(numberField(dialog, 'IAT histogram lo')).toHaveValue(0);
    expect(numberField(dialog, 'IAT histogram hi')).toHaveValue(1.024);
    await typeInto(user, dialog, 'Histogram bins', '64');
    await typeInto(user, dialog, 'Histogram hi', '');
    await typeInto(user, dialog, 'IAT histogram bins', '');
    await confirmModal(user);
    expect(modalAlert()).toBeNull();
    const reopened = await openHistogram(user, 1);
    expect(numberField(reopened, 'Histogram bins')).toHaveValue(64);
    expect(numberField(reopened, 'Histogram hi')).toHaveValue(2.048);
    expect(numberField(reopened, 'IAT histogram bins')).toHaveValue(16);
    await closeModal(user);
    await clickSave(user);
    await findToast('Settings saved successfully.');
    const { histogram, iat_histogram } = must(storedLinks()[0]).config;
    expect(histogram).toStrictEqual({ bins: 64, lo_ns: 1024, hi_ns: 2048 });
    expect(iat_histogram).toStrictEqual({ bins: 16, lo_ns: 0, hi_ns: 1024 });
  });
});

describe('Import and Export', () => {
  const stubDownload = () => {
    const blobs: Blob[] = [];
    const files: string[] = [];
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      writable: true,
      value: (b: Blob) => {
        blobs.push(b);
        return 'blob:test';
      },
    });
    Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, writable: true, value: () => undefined });
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function click(this: HTMLAnchorElement) {
      files.push(this.download);
    });
    return { blobs, files };
  };
  const textOf = (b: Blob) =>
    new Promise<string>((resolveText, reject) => {
      const r = new FileReader();
      r.onload = () => resolveText(String(r.result));
      r.onerror = () => reject(r.error);
      r.readAsText(b);
    });
  const upload = (user: UserEvent, content: unknown, name = 'links.json') =>
    user.upload(
      screen.getByLabelText('Import links file') as HTMLInputElement,
      new File([typeof content === 'string' ? content : JSON.stringify(content)], name, { type: 'application/json' }),
    );
  const confirmImport = async (user: UserEvent) =>
    user.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Confirm import' }));
  const importedLinks = (): Link[] => [fx.loopLink({ id: 9 }), { ...fx.link({ id: 12 }), avg_frame_size_byte: 512 }];

  it('choosing a file asks for confirmation first; Cancel writes nothing', async () => {
    const user = await page();
    const before = rawStore();
    await upload(user, { version: 1, links: importedLinks() });
    const dialog = await screen.findByRole('dialog');
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(rawStore()).toBe(before);
    expect(queryToast()).toBeNull();
    expect(portsOfRows()).toEqual(pairs([fx.link()]));
  });

  it('Confirm replaces the store AND the draft (unsaved rows too) with the file: ids and frame size kept, settings, workers and names dropped', async () => {
    const user = await page();
    await addLink(user);
    const file = {
      version: 1,
      links: importedLinks().map((l) => ({ ...l, name: 'old', worker: { lcore: 3 } })),
      settings: { main_lcore: 2 },
      presets: [],
    };
    await upload(user, file);
    await confirmImport(user);
    await findToast('Settings imported successfully.');
    expect(portsOfRows()).toEqual(pairs(importedLinks()));
    expect(storedDoc()).toEqual({ version: 1, links: importedLinks() });
    expect(Object.keys(storedDoc()).sort()).toEqual(['links', 'version']);
  });

  it('an import replaces an edited draft even when the file equals the stored links', async () => {
    const user = await page();
    await addLink(user);
    await upload(user, { version: 1, links: [fx.link()] });
    await confirmImport(user);
    await findToast('Settings imported successfully.');
    expect(portsOfRows()).toEqual(pairs([fx.link()]));
  });

  it('an empty links list is valid: it empties the table', async () => {
    const user = await page();
    await upload(user, { version: 1, links: [] });
    await confirmImport(user);
    await findToast('Settings imported successfully.');
    expect(screen.getByText('No links configured yet.')).toBeInTheDocument();
    expect(storedLinks()).toEqual([]);
  });

  it.each([
    ['a file that is not JSON', '{nope', /JSON/],
    ['an empty file', '', /JSON/],
    ['a file of another shape', { version: 2, links: [] }, /links file/],
    [
      'a link the form would not accept (named by position)',
      { version: 1, links: [fx.link(), { ...fx.link({ id: 2 }), ingress: '' }] },
      /Link 2: /,
    ],
  ])('%s: a danger toast, no dialog, nothing changes', async (_what, content, message) => {
    const user = await page();
    const before = rawStore();
    await upload(user, content);
    await findToast(message);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(rawStore()).toBe(before);
    expect(portsOfRows()).toEqual(pairs([fx.link()]));
  });

  it('Export downloads the STORED links as dpds-links-<date>.json, never the unsaved draft', async () => {
    const { blobs, files } = stubDownload();
    const user = await page();
    await addLink(user);
    await user.click(exportButton());
    await waitFor(() => expect(blobs).toHaveLength(1));
    expect(files[0]).toMatch(/^dpds-links-\d{4}-\d{2}-\d{2}\.json$/);
    expect(JSON.parse(await textOf(must(blobs[0])))).toEqual({ version: 1, links: [fx.link()] });
  });

  it('Export of a corrupt store downloads the stored text unchanged', async () => {
    const { blobs } = stubDownload();
    seedRaw('{not json');
    const user = await renderSettings();
    await user.click(exportButton());
    await waitFor(() => expect(blobs).toHaveLength(1));
    expect(await textOf(must(blobs[0]))).toBe('{not json');
    expect(rawStore()).toBe('{not json');
  });

  it('a failing download is reported', async () => {
    Object.defineProperty(URL, 'createObjectURL', {
      configurable: true,
      writable: true,
      value: () => {
        throw new Error('disk full');
      },
    });
    const user = await page();
    await user.click(exportButton());
    await findToast('disk full');
  });
});

describe('pending restart and Load running configuration', () => {
  const run = (links: Link[], state: 'running' | 'starting' = 'running') =>
    api.getEmulation.mockResolvedValue(fx.emulation(state, { config: fx.runningConfig(links) }));
  const settle = async () => {
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
  };
  const notice = () => screen.queryByTestId('pending-restart');
  const marker = (n: number) => within(rowOf(n)).queryByTestId('link-pending-restart');

  it('a stored link that differs from the running one raises the notice and marks only its row', async () => {
    run([fx.link(), fx.linkRev()]);
    seedLinks([{ ...fx.link(), config: { ...fx.config(), buffer_capacity: 5 } }, fx.linkRev()]);
    await renderSettings();
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    await waitFor(() => expect(marker(1)).not.toBeNull());
    expect(marker(2)).toBeNull();
  });

  it('nothing shows while the stored links equal the running ones; an unsaved draft edit changes nothing, Save does', async () => {
    run([fx.link(), fx.linkRev()]);
    const user = await page([fx.link(), fx.linkRev()]);
    await settle();
    expect(notice()).toBeNull();
    await openSettings(user, 1);
    await typeInto(user, body(), 'Buffer capacity', '5', 'packets');
    await confirmModal(user);
    await settle();
    expect(notice()).toBeNull();
    await clickSave(user);
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
    expect(marker(1)).not.toBeNull();
  });

  it('with a running emulation and no stored links "Load running configuration" fills the table and the store, then goes away', async () => {
    run([fx.link(), fx.linkRev()]);
    seedLinks([]);
    const user = await renderSettings();
    await user.click(await screen.findByRole('button', { name: 'Load running configuration' }));
    await waitFor(() => expect(portsOfRows()).toEqual(pairs([fx.link(), fx.linkRev()])));
    expect(storedLinks().map((l) => [l.id, l.ingress])).toEqual([
      [1, fx.link().ingress],
      [2, fx.linkRev().ingress],
    ]);
    await settle();
    expect(screen.queryByRole('button', { name: 'Load running configuration' })).not.toBeInTheDocument();
    expect(notice()).toBeNull();
  });

  it('it is not offered while links are stored', async () => {
    run([fx.link(), fx.linkRev()]);
    await page([fx.link()]);
    await settle();
    expect(screen.queryByRole('button', { name: 'Load running configuration' })).not.toBeInTheDocument();
  });
});

describe('a corrupt store', () => {
  it('shows the alert with a Reset, no rows, disabled Add link, Save, Reset and Import; Reset empties the store and the page works again', async () => {
    seedRaw('{not json');
    const user = await renderSettings();
    expect(await screen.findByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(screen.queryAllByLabelText(/^RX port of link/)).toHaveLength(0);
    expect(addLinkButton()).toBeDisabled();
    expect(saveButton()).toBeDisabled();
    expect(resetButton()).toBeDisabled();
    expect(importButton()).toBeDisabled();
    expect(exportButton()).toBeEnabled();
    expect(rawStore()).toBe('{not json');
    await user.click(screen.getByRole('button', { name: 'Reset local configuration' }));
    await waitFor(() => expect(addLinkButton()).toBeEnabled());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(rawStore() === null || storedDoc().links.length === 0).toBe(true);
  });

  it('a stored link the form would not accept makes the document corrupt too, and Start is disabled', async () => {
    const raw = JSON.stringify({ version: 1, links: [fx.link(), { ...fx.link({ id: 2 }), ingress: '' }] });
    seedRaw(raw);
    renderApp('/');
    expect(await screen.findByText(/local configuration of this browser cannot be read/i)).toBeInTheDocument();
    expect(await screen.findByRole('button', { name: /^start$/i })).toBeDisabled();
    expect(api.startEmulation).not.toHaveBeenCalled();
    expect(rawStore()).toBe(raw);
  });
});

describe('the InfoBox next to a field label', () => {
  const LINK_LABELS: HelpKey[] = [
    'Buffer capacity',
    'Seed',
    'Average frame size',
    'Delay distribution',
    'Mean delay',
    'Delay std',
    'Delay accuracy',
    'Half-life period',
    'Initialization rate',
    'Limiter mode',
    'Rate',
    'Loss model',
    'Gilbert-Elliot p',
    'Gilbert-Elliot r',
    'Gilbert-Elliot k',
    'Gilbert-Elliot h',
  ];
  const HISTOGRAM_LABELS: HelpKey[] = [
    'Delay statistics',
    'Histogram bins',
    'Histogram lo',
    'Histogram hi',
    'IAT histogram bins',
    'IAT histogram lo',
    'IAT histogram hi',
  ];
  const about = (label: string) => screen.getByRole('button', { name: `About ${label}` });
  /** Opens the box of `label`, returns its text and closes it again. */
  async function infoOf(user: UserEvent, label: string): Promise<string> {
    await user.click(about(label));
    const info = must(infoDialog(), 'the Info modal');
    expect(within(info).getByRole('heading', { level: 5, name: label })).toBeInTheDocument();
    const text = info.querySelector('p')?.textContent ?? '';
    await user.click(footerButton(info, 'Close'));
    expect(infoDialog()).toBeNull();
    return text;
  }

  it('every field of the Link settings modal has one that shows its help text', async () => {
    const user = await newFullLink();
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    for (const label of LINK_LABELS) expect(await infoOf(user, label), label).toBe(HELP[label]);
  });

  it('every field of the Histogram modal has one that shows its help text', async () => {
    const user = await page();
    await openHistogram(user, 1);
    for (const label of HISTOGRAM_LABELS) expect(await infoOf(user, label), label).toBe(HELP[label]);
  });

  it('opens on top of the settings modal; closing it returns to it unchanged, and a click neither submits nor writes', async () => {
    const user = await page();
    await openSettings(user, 1);
    const writes = watchStoreWrites();
    await typeInto(user, body(), 'Seed', '42');
    await user.click(about('Mean delay'));
    expect(infoDialog()).not.toBeNull();
    expect(linkDialog()).not.toBeNull();
    await user.click(footerButton(must(infoDialog()), 'Close'));
    expect(infoDialog()).toBeNull();
    expect(openModal()).not.toBeNull();
    expect(screen.getByLabelText('Seed')).toHaveValue(42);
    expect(writes()).toBe(0);
  });
});
