import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import * as fx from '../test/fixtures';
import { linkPage, PAGES } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  addStages,
  numberField,
  pickUnit,
  rawStore,
  renderApp,
  section,
  seedLinks,
  storedLinks,
  typeInto,
  typeRate,
  unitOf,
  watchStoreWrites,
} from '../test/helpers';
import { HELP } from './fieldHelp';
import { must } from '../test/must';

// Spec 26 (user decisions 2026-11-04). Written to the spec, not to an implementation.
//
// 1. The Gilbert-Elliott "starts in good state" flag is gone from the UI: no checkbox, no tooltip, no inspect row, no
//    form member, no DTO member. The GilbertElliot DTO the UI sends is exactly {type, p, r, k, h}.
// 2. A link name must follow the controller's rule (1 to 32 characters, each of A-Z a-z 0-9 _ . -). Saving a link whose
//    name breaks it is refused with an alert naming "Name" and stating the rule (tester's choice for "stating": the text
//    holds the number 32 and the words "letters" or the range "A-Z"); nothing is written. (Spec27 removed presets, and with them the preset-name freedom.)
// 3. A cross-family unit change remembers the value it converted from (text + unit) until the number is edited.
// 4. The Initialization rate minimum (>= 1 pps) is checked on the exact converted value, before rounding.

const body = () => document.body;
const FRAME = 'Average frame size';
const typeFrame = (user: UserEvent, text: string) => typeInto(user, body(), FRAME, text);
const shown = (label: string) => numberField(body(), label).value;

const frontendDir = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const source = (rel: string) => readFileSync(resolve(frontendDir, 'src', rel), 'utf8');

// ---- 1. Gilbert-Elliott start state ----------------------------------------------------------------------------

const GE: ConfigDto['loss_model'] = {
  type: 'GilbertElliot',
  p: 0.01,
  r: 0.2,
  k: 0.9,
  h: 0.3,
};
const GE_KEYS = ['h', 'k', 'p', 'r', 'type'];

describe.each(PAGES)('$name: the Gilbert-Elliott start state is gone', (page: ConfigPage) => {
  it('the Dropper offers p, r, k and h and no checkbox of any kind (no start-state control)', async () => {
    const user = await page.openFull();
    await user.selectOptions(within(section('Dropper')).getByLabelText('Loss model'), 'GilbertElliot');
    const d = section('Dropper');
    for (const n of ['p', 'r', 'k', 'h']) expect(within(d).getByLabelText(`Gilbert-Elliot ${n}`)).toBeInTheDocument();
    expect(within(d).queryByLabelText(/good state/i)).not.toBeInTheDocument();
    expect(within(d).queryAllByRole('checkbox')).toHaveLength(0);
    expect(screen.queryByText(/starts in good state/i)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /About Gilbert-Elliot starts/i })).not.toBeInTheDocument();
  });

  it('a new Gilbert-Elliott form saves exactly {type, p, r, k, h}', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await user.selectOptions(within(section('Dropper')).getByLabelText('Loss model'), 'GilbertElliot');
    await page.save(user);
    const loss = must(page.created().loss_model);
    expect(loss.type).toBe('GilbertElliot');
    expect(Object.keys(loss).sort()).toEqual(GE_KEYS);
  });

  it('a stored Gilbert-Elliott config opens with its values and is saved back as exactly {type, p, r, k, h}', async () => {
    const user = await page.openStored({ ...fx.config(), loss_model: GE });
    const d = section('Dropper');
    expect(within(d).getByLabelText('Gilbert-Elliot p')).toHaveValue(0.01);
    expect(within(d).getByLabelText('Gilbert-Elliot h')).toHaveValue(0.3);
    expect(within(d).queryByLabelText(/good state/i)).not.toBeInTheDocument();
    await page.save(user);
    expect(page.updated().loss_model).toEqual(GE);
    expect(Object.keys(must(page.updated().loss_model)).sort()).toEqual(GE_KEYS);
  });
});

describe('Gilbert-Elliott start state: no trace in the sources the UI sends and shows from', () => {
  it('the help texts have no entry for it', () => {
    expect(Object.keys(HELP).filter((k) => /good state/i.test(k))).toEqual([]);
  });

  it.each([
    'api/types.ts',
    'pages/linkForm.ts',
    'pages/ConfigFieldset.tsx',
    'pages/LinkInspect.tsx',
    'pages/fieldHelp.ts',
  ])('%s does not mention state_is_good, ge_good or the checkbox label', (file) => {
    expect(source(file)).not.toMatch(/state_is_good|ge_good|starts in good state/i);
  });
});

// ---- 2. Link names ----------------------------------------------------------------------------------------------

async function fillLink(user: UserEvent, name: string) {
  const el = screen.getByLabelText('Name');
  await user.clear(el);
  if (name !== '') await user.type(el, name);
  await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.0');
  await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.1');
  if (screen.queryByLabelText('Initialization rate')) await typeRate(user);
}

const save = (user: UserEvent) => user.click(screen.getByRole('button', { name: /^save$/i }));

describe('link editor: the name follows the controller rule (1 to 32 of A-Z a-z 0-9 _ . -)', () => {
  const BAD: [string, string][] = [
    ['an inner space', 'my link'],
    ['a slash', 'a/b'],
    ['a colon', 'a:b'],
    ['an exclamation mark', 'link!'],
    ['an at sign', 'a@b'],
    ['a plus sign', 'a+b'],
    ['a comma', 'a,b'],
    ['a non-ASCII letter', 'länk'],
    ['a non-ASCII letter only', 'é'],
    ['a micro sign', 'aµs'],
    ['33 characters', 'a'.repeat(33)],
    ['64 characters', 'a'.repeat(64)],
  ];

  it.each(BAD)('%s is refused with an alert naming "Name" and the rule; nothing is written', async (_what, name) => {
    const user = await linkPage.openNew();
    await fillLink(user, name);
    await save(user);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/\bName\b/);
    expect(alert).toHaveTextContent(/32/);
    expect(alert).toHaveTextContent(/letters|A-Z/i);
    expect(linkPage.requests()).toBe(0);
    expect(rawStore()).toBeNull();
    expect(screen.getByRole('heading', { level: 1, name: 'New link' })).toBeInTheDocument();
    expect(screen.getByLabelText('Name')).toHaveValue(name);
  });

  const GOOD: [string, string][] = [
    ['one character', 'a'],
    ['a digit only', '7'],
    ['all allowed punctuation', 'A_b.c-9'],
    ['upper case', 'ABC-XYZ'],
    ['a leading dot', '.hidden'],
    ['a leading dash', '-x'],
    ['exactly 32 characters', 'x'.repeat(32)],
    ['a mixed 32 characters', 'Link_1.2-bZ0123456789abcdefghij.-_'.slice(0, 32)],
  ];

  it.each(GOOD)('%s is accepted and stored as typed', async (_what, name) => {
    const user = await linkPage.openNew();
    await fillLink(user, name);
    await save(user);
    expect(await screen.findByRole('heading', { level: 1, name: 'Settings' })).toBeInTheDocument();
    expect(storedLinks()).toHaveLength(1);
    expect(storedLinks()[0].name).toBe(name);
  });

  it('an empty name is still refused with an alert naming "Name"', async () => {
    const user = await linkPage.openNew();
    await fillLink(user, '');
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/\bName\b/);
    expect(linkPage.requests()).toBe(0);
    expect(rawStore()).toBeNull();
  });

  it('a refused name keeps every other typed value and the corrected name saves', async () => {
    const user = await linkPage.openNew();
    await fillLink(user, 'bad name');
    await typeInto(user, body(), 'Buffer capacity', '777', 'packets');
    await save(user);
    await screen.findByRole('alert');
    expect(numberField(body(), 'Buffer capacity')).toHaveValue(777);
    expect(unitOf(body(), 'Buffer capacity')).toBe('packets');
    expect((screen.getByLabelText('Ingress') as HTMLSelectElement).value).toBe('0000:65:00.0');
    expect(linkPage.requests()).toBe(0);
    const name = screen.getByLabelText('Name');
    await user.clear(name);
    await user.type(name, 'good-name');
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(storedLinks()[0].name).toBe('good-name');
    expect(storedLinks()[0].config.buffer_capacity).toBe(777);
  });

  it('the duplicate-name refusal stays (checked with a name that follows the rule)', async () => {
    seedLinks([fx.link({ name: 'taken' })]);
    const user = userEvent.setup();
    renderApp('/settings/new');
    await screen.findByRole('heading', { level: 1, name: 'New link' });
    await waitFor(() => expect(screen.getAllByRole('option', { name: /enp101s0f0/ })).not.toHaveLength(0));
    await addStages(user);
    const writes = watchStoreWrites();
    await user.type(screen.getByLabelText('Name'), 'taken');
    await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.1');
    await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.0');
    await typeRate(user);
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/already exists/i);
    expect(writes()).toBe(0);
  });

  it('editing: renaming a stored link to an invalid name is refused, the stored name is unchanged', async () => {
    const user = await linkPage.openStored(fx.config());
    const name = screen.getByLabelText('Name');
    await user.clear(name);
    await user.type(name, 'new name');
    await save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/\bName\b/);
    expect(linkPage.requests()).toBe(0);
    expect(storedLinks()[0].name).toBe('a-b');
  });

  // Spec 27b item 1: a stored link whose name breaks the rule is no longer opened and refused on Save; the document is corrupt
  // (alert with Reset, stored text untouched, no editor). Renaming a valid link to a bad name is refused in the test above.
  it('editing: a stored link whose name breaks the rule makes the document corrupt: the alert, no form, nothing written', async () => {
    seedLinks([fx.link({ name: 'old name' })]);
    const before = rawStore();
    const writes = watchStoreWrites();
    renderApp('/settings/1');
    expect(await screen.findByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(screen.getByRole('button', { name: /^reset/i })).toBeInTheDocument();
    expect(screen.queryByLabelText('Name')).not.toBeInTheDocument();
    expect(writes()).toBe(0);
    expect(rawStore()).toBe(before);
  });

  it('editing: a stored link with a valid name still saves', async () => {
    const user = await linkPage.openStored(fx.config());
    await save(user);
    await screen.findByRole('heading', { level: 1, name: 'Settings' });
    expect(storedLinks()[0].name).toBe('a-b');
  });
});

// ---- 3. No drift when switching rate units back and forth -------------------------------------------------------

// 1518 B frame: 1e11 bit/s / ((1518 + 20) x 8) = 8127438.23... pps; 7.7 Gbit/s = 625812.4 pps.
describe.each(PAGES)('$name: a cross-family unit change remembers the value it converted from', (page: ConfigPage) => {
  async function open() {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    return user;
  }
  const init = (user: UserEvent, text: string, unit: string) =>
    typeInto(user, body(), 'Initialization rate', text, unit);
  const pick = (user: UserEvent, label: string, unit: string) => pickUnit(user, body(), label, unit);

  it('Initialization rate: 100 Gbit/s -> Mpps shows 8.127 -> Gbit/s shows exactly 100 again', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
  });

  it('Rate: 7.7 Gbit/s -> Mpps shows 0.626 -> Gbit/s shows exactly 7.7 again (not 7.702)', async () => {
    const user = await open();
    await typeInto(user, body(), 'Rate', '7.7', 'Gbit/s');
    await pick(user, 'Rate', 'Mpps');
    expect(shown('Rate')).toBe('0.626');
    await pick(user, 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('7.7');
  });

  it('Rate: 8.127 Mpps typed -> Gbit/s -> Mpps shows exactly 8.127 again', async () => {
    const user = await open();
    await typeInto(user, body(), 'Rate', '8.127', 'Mpps');
    await pick(user, 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('99.995');
    await pick(user, 'Rate', 'Mpps');
    expect(shown('Rate')).toBe('8.127');
  });

  it('the round trip can be repeated: the remembered value survives returning to the original unit', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    for (let i = 0; i < 3; i += 1) {
      await pick(user, 'Initialization rate', 'Mpps');
      expect(shown('Initialization rate')).toBe('8.127');
      await pick(user, 'Initialization rate', 'Gbit/s');
      expect(shown('Initialization rate')).toBe('100');
    }
  });

  it('further changes convert from the remembered value, also within the other family (kpps, pps)', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127');
    await pick(user, 'Initialization rate', 'kpps');
    expect(shown('Initialization rate')).toBe('8127.438'); // from 100 Gbit/s, not 8.127 Mpps shifted to 8127
    await pick(user, 'Initialization rate', 'pps');
    expect(shown('Initialization rate')).toBe('8127438.231');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
  });

  it('further changes convert from the remembered value into a third cross unit: Mpps -> Mbit/s shows 100000', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await pick(user, 'Initialization rate', 'Mbit/s');
    expect(shown('Initialization rate')).toBe('100000'); // not 99994.608
    await pick(user, 'Initialization rate', 'kbit/s');
    expect(shown('Initialization rate')).toBe('100000000'); // decimal-exact within the family
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
  });

  it('the shown text after a cross-family change keeps the 3-decimal rounding', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127'); // not 8.127438231...
  });

  it('save uses the remembered value: 100 Gbit/s -> Mpps -> save stores 8127438 pps, not 8127000', async () => {
    const user = await open();
    await page.ready(user);
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127');
    await page.save(user);
    expect(must(page.created().delayer).initialization_rate_pps).toBe(8127438);
  });

  it('save uses the remembered value after several unit changes too (kpps)', async () => {
    const user = await open();
    await page.ready(user);
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await pick(user, 'Initialization rate', 'kpps');
    await page.save(user);
    expect(must(page.created().delayer).initialization_rate_pps).toBe(8127438);
  });

  it('the limiter rate is saved from the remembered value: 7.7 Gbit/s -> Mpps -> save stores 7700000000 bit/s', async () => {
    const user = await open();
    await page.ready(user);
    await typeInto(user, body(), 'Rate', '7.7', 'Gbit/s');
    await pick(user, 'Rate', 'Mpps');
    expect(shown('Rate')).toBe('0.626');
    await page.save(user);
    expect(must(page.created().limiter).rate_bps).toBe(7.7e9);
  });

  it('typing into the number drops the remembered value: the typed number is what counts (convert)', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '8');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('98.432'); // 8 Mpps, not 100
  });

  it('typing the very digits that are shown also drops it (any edit does)', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '8.127');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('99.995'); // 8.127 Mpps, not 100
  });

  it('typing into the number drops the remembered value (save)', async () => {
    const user = await open();
    await page.ready(user);
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '8');
    await page.save(user);
    expect(must(page.created().delayer).initialization_rate_pps).toBe(8000000);
  });

  it('typing after a within-family change on top of the remembered value drops it as well', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await pick(user, 'Initialization rate', 'kpps');
    await typeInto(user, body(), 'Initialization rate', '5');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('0.062'); // 5 kpps x 12304 = 0.06152 Gbit/s
  });

  it('a new cross-family change after a typed number remembers the new value', async () => {
    const user = await open();
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '8');
    await pick(user, 'Initialization rate', 'Gbit/s'); // 98.432, remembers 8 Mpps
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8');
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('98.432');
  });

  it('the two rate fields remember independently', async () => {
    const user = await open();
    await page.ready(user);
    await init(user, '100', 'Gbit/s');
    await typeInto(user, body(), 'Rate', '7.7', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await pick(user, 'Rate', 'Mpps');
    await pick(user, 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('7.7');
    expect(shown('Initialization rate')).toBe('8.127'); // untouched by the other field
    await pick(user, 'Initialization rate', 'Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
  });

  it('within-family changes stay decimal-exact when nothing was converted across', async () => {
    const user = await open();
    await init(user, '2.5', 'Mpps');
    await pick(user, 'Initialization rate', 'kpps');
    expect(shown('Initialization rate')).toBe('2500');
    await pick(user, 'Initialization rate', 'pps');
    expect(shown('Initialization rate')).toBe('2500000');
    await pick(user, 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('2.5');
  });

  it('the remembered value is form state only: the saved link holds no trace and reopens as the stored pps', async () => {
    const user = await open();
    await page.ready(user);
    await init(user, '100', 'Gbit/s');
    await pick(user, 'Initialization rate', 'Mpps');
    await page.save(user);
    const raw = rawStore() ?? '';
    expect(raw).not.toMatch(/Gbit|remember|unit/i);
    const entry = storedLinks()[0];
    expect(Object.keys(entry.config.delayer ?? {}).sort()).toEqual([
      'activate_correlation',
      'distribution',
      'half_life_period_ns',
      'initialization_rate_pps',
      'mean_ns',
      'std_ns',
    ]);
  });

  it('a form reopened from the stored value remembers nothing (a stored 8127438 pps shows 8127.438 kpps)', async () => {
    const user = await page.openStored(
      fx.config({
        delayer: {
          ...must(fx.config().delayer),
          initialization_rate_pps: 8127438,
        },
      }),
    );
    await typeFrame(user, '1518');
    expect(shown('Initialization rate')).toBe('8127.438');
    expect(unitOf(body(), 'Initialization rate')).toBe('kpps');
  });
});

// The user's own example (2026-11-04): frame 1518 B, 100 Gbit/s -> Mpps (the field shows 8.127), then the user EDITS the
// number. Whatever the edit, the remembered 100 Gbit/s is gone and the typed number is what counts. Both rate fields.
// 1 Mpps at 1518 B is 12304000000 bit/s = 12.304 Gbit/s.
const RATE_FIELDS: [string, (c: ConfigDto) => number | undefined, number, string][] = [
  // [label, stored value of the config, stored value for 1 Mpps, what a missing value is refused as]
  ['Initialization rate', (c) => c.delayer?.initialization_rate_pps, 1000000, 'Initialization rate'],
  ['Rate', (c) => c.limiter?.rate_bps, 12304000000, 'Rate'],
];

describe.each(PAGES)(
  '$name: editing the number after a cross-family switch discards the remembered value',
  (page: ConfigPage) => {
    async function switched(label: string) {
      const user = await page.openFull();
      await page.ready(user);
      await typeFrame(user, '1518');
      await typeInto(user, body(), label, '100', 'Gbit/s');
      await pickUnit(user, body(), label, 'Mpps');
      expect(shown(label)).toBe('8.127');
      return user;
    }

    describe.each(RATE_FIELDS)('%s', (label, stored, forOneMpps, refusedAs) => {
      it('types 1 -> back to Gbit/s shows 12.304, never 100', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '1');
        await pickUnit(user, body(), label, 'Gbit/s');
        expect(unitOf(body(), label)).toBe('Gbit/s');
        expect(shown(label)).toBe('12.304');
      });

      it('types 1 -> save stores the 1 Mpps conversion, not 100 Gbit/s', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '1');
        await page.save(user);
        expect(stored(page.created())).toBe(forOneMpps);
      });

      it('types 1 -> back to Gbit/s -> save stores the same value', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '1');
        await pickUnit(user, body(), label, 'Gbit/s');
        await page.save(user);
        expect(stored(page.created())).toBe(forOneMpps);
      });

      it('retyping the same text (8.127) also drops it: back to Gbit/s shows 99.995, not 100', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '8.127');
        await pickUnit(user, body(), label, 'Gbit/s');
        expect(shown(label)).toBe('99.995');
      });

      it('retyping the same text (8.127) also drops it: save stores 8.127 Mpps, not 100 Gbit/s', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '8.127');
        await page.save(user);
        expect(stored(page.created())).toBe(label === 'Rate' ? 99994608000 : 8127000);
      });

      it('clearing the field drops it: back to Gbit/s the field stays empty, not 100', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '');
        await pickUnit(user, body(), label, 'Gbit/s');
        expect(unitOf(body(), label)).toBe('Gbit/s');
        expect(shown(label)).toBe('');
      });

      it('clearing the field drops it: save is refused naming the field, 100 Gbit/s is not saved', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '');
        await page.save(user);
        expect(await screen.findByRole('alert')).toHaveTextContent(new RegExp(`\\b${refusedAs}\\b`));
        expect(page.requests()).toBe(0);
        expect(rawStore()).toBeNull();
      });

      it('clearing and then typing 1 behaves like typing 1: Gbit/s shows 12.304', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '');
        await typeInto(user, body(), label, '1');
        await pickUnit(user, body(), label, 'Gbit/s');
        expect(shown(label)).toBe('12.304');
      });

      it('a small edit (8.127 -> 8.12) drops it too', async () => {
        const user = await switched(label);
        await typeInto(user, body(), label, '8.12');
        await pickUnit(user, body(), label, 'Gbit/s');
        expect(shown(label)).toBe('99.908'); // 8.12 Mpps x 12304 = 99.90848 Gbit/s
      });
    });
  },
);

// ---- 3b. Editing the frame size drops the remembered values (spec26 review, 2026-11-04) -------------------------------

// A cross-family switch makes a rate field show a rounded number while it remembers the exact value it converted from. Any
// edit of "Average frame size" (change, clear or retype) drops the remembered value of BOTH rate fields: the shown text and unit
// stay as they are and from then on the SHOWN value is what counts (it is what the user sees, and the old frame no longer
// applies). Before this rule, save converted the remembered 100 Gbit/s with the NEW frame (148809524 pps while 8.127 Mpps was
// shown), clearing the frame made save fail with "required to convert ... from Gbit/s", and Mpps -> kpps blanked the field.
// 1518 B frame: 8.127 Mpps shown for 100 Gbit/s; 8.127 Mpps shown as Gbit/s is 99.995 (exactly 99.994608).
describe.each(PAGES)('$name: editing the frame size drops the remembered rate values', (page: ConfigPage) => {
  /** Frame 1518, Initialization rate 100 Gbit/s switched to Mpps (shows 8.127, remembers 100 Gbit/s). */
  async function initRemembered() {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await page.ready(user);
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127');
    return user;
  }
  /** Frame 1518, limiter Rate 8.127 Mpps switched to Gbit/s (shows 99.995, remembers 8.127 Mpps). */
  async function rateRemembered() {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await page.ready(user);
    await typeInto(user, body(), 'Rate', '8.127', 'Mpps');
    await pickUnit(user, body(), 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('99.995');
    return user;
  }

  describe('Initialization rate', () => {
    it('after a changed frame size the shown 8.127 Mpps is saved: 8127000 pps (not 148809524)', async () => {
      const user = await initRemembered();
      await typeFrame(user, '64');
      expect(shown('Initialization rate')).toBe('8.127');
      expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(8127000);
    });

    it('retyping the very same frame size drops it too (any edit does): 8127000 pps, not 8127438', async () => {
      const user = await initRemembered();
      await typeFrame(user, '1518');
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(8127000);
    });

    it('after clearing the frame size the Mpps value saves without a frame-size error', async () => {
      const user = await initRemembered();
      await typeFrame(user, '');
      expect(shown('Initialization rate')).toBe('8.127');
      expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
      await page.save(user);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(must(page.created().delayer).initialization_rate_pps).toBe(8127000);
    });

    it('after clearing the frame size Mpps -> kpps shows 8127 (decimal-exact, not blank) and -> pps 8127000', async () => {
      const user = await initRemembered();
      await typeFrame(user, '');
      await pickUnit(user, body(), 'Initialization rate', 'kpps');
      expect(shown('Initialization rate')).toBe('8127');
      await pickUnit(user, body(), 'Initialization rate', 'pps');
      expect(shown('Initialization rate')).toBe('8127000');
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(8127000);
    });

    it('a later cross-family switch converts the SHOWN value with the new frame: 8.127 Mpps at 64 B is 5.461 Gbit/s', async () => {
      const user = await initRemembered();
      await typeFrame(user, '64');
      await pickUnit(user, body(), 'Initialization rate', 'Gbit/s');
      expect(shown('Initialization rate')).toBe('5.461'); // 8127000 x (64 + 20) x 8 bit, not 100
    });

    it('the remembered value is dropped only by a frame edit: without one, save still stores 8127438', async () => {
      const user = await initRemembered();
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(8127438);
    });
  });

  describe('limiter Rate', () => {
    it('after a changed frame size the shown 99.995 Gbit/s is saved: 99995000000 bit/s (not 8.127 Mpps converted at 64 B)', async () => {
      const user = await rateRemembered();
      await typeFrame(user, '64');
      expect(shown('Rate')).toBe('99.995');
      expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
      await page.save(user);
      expect(must(page.created().limiter).rate_bps).toBe(99995000000);
    });

    it('retyping the very same frame size drops it too: 99995000000 bit/s, not 99994608000', async () => {
      const user = await rateRemembered();
      await typeFrame(user, '1518');
      await page.save(user);
      expect(must(page.created().limiter).rate_bps).toBe(99995000000);
    });

    it('after clearing the frame size the Gbit/s value saves without a frame-size error', async () => {
      const user = await rateRemembered();
      await typeFrame(user, '');
      expect(shown('Rate')).toBe('99.995');
      await page.save(user);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      expect(must(page.created().limiter).rate_bps).toBe(99995000000);
    });

    it('after clearing the frame size Gbit/s -> Mbit/s shows 99995 (decimal-exact, not blank) and -> bit/s 99995000000', async () => {
      const user = await rateRemembered();
      await typeFrame(user, '');
      await pickUnit(user, body(), 'Rate', 'Mbit/s');
      expect(shown('Rate')).toBe('99995');
      await pickUnit(user, body(), 'Rate', 'bit/s');
      expect(shown('Rate')).toBe('99995000000');
    });

    it('a later cross-family switch converts the SHOWN value with the new frame: 99.995 Gbit/s at 64 B is 148.8 Mpps', async () => {
      const user = await rateRemembered();
      await typeFrame(user, '64');
      await pickUnit(user, body(), 'Rate', 'Mpps');
      expect(shown('Rate')).toBe('148.802'); // 99995000000 / 672 = 148802083.3 pps, not 8.127
    });
  });

  it('one frame edit drops the remembered values of BOTH fields at once', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await page.ready(user);
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    await typeInto(user, body(), 'Rate', '8.127', 'Mpps');
    await pickUnit(user, body(), 'Rate', 'Gbit/s');
    await typeFrame(user, '');
    await page.save(user);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(must(page.created().delayer).initialization_rate_pps).toBe(8127000);
    expect(must(page.created().limiter).rate_bps).toBe(99995000000);
  });

  it('the new rule leaves a field without a remembered value alone (same-family values and typed numbers)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await page.ready(user);
    await typeInto(user, body(), 'Initialization rate', '5', 'Mpps');
    await typeFrame(user, '64');
    await typeFrame(user, '');
    await page.save(user);
    expect(must(page.created().delayer).initialization_rate_pps).toBe(5000000);
  });
});

// ---- 4. Minimum check before rounding ---------------------------------------------------------------------------

// 1518 B: 1 pps = 12304 bit/s, so x Mbit/s is x / 0.012304 pps. Values are typed in Mbit/s to stay exact.
describe.each(PAGES)(
  '$name: the Initialization rate minimum is checked on the exact converted value',
  (page: ConfigPage) => {
    async function typeConverted(user: UserEvent, text: string, unit = 'Mbit/s') {
      await page.ready(user);
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', text, unit);
    }

    it.each([
      ['0.5 pps', '0.006152'],
      ['0.6 pps', '0.0073824'],
      ['0.99 pps', '0.01218096'],
      ['0.999999 pps', '0.012303987696'],
    ])('a value converting to %s is refused with an alert naming the field; nothing is written', async (_pps, mbit) => {
      const user = await page.openFull();
      await typeConverted(user, mbit);
      await page.save(user);
      expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
      expect(page.requests()).toBe(0);
      expect(rawStore()).toBeNull();
      expect(shown('Initialization rate')).toBe(mbit);
    });

    it.each([
      ['exactly 1 pps', '0.012304', 1],
      ['1.4 pps', '0.0172256', 1],
      ['1.5 pps', '0.018456', 2],
      ['1.6 pps', '0.0196864', 2],
    ])('a value converting to %s is accepted and stored rounded as %d', async (_pps, mbit, stored) => {
      const user = await page.openFull();
      await typeConverted(user, mbit);
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(stored);
    });

    it('0.6 pps typed in the Gbit/s unit is refused as well (0.0000073824 Gbit/s)', async () => {
      const user = await page.openFull();
      await typeConverted(user, '0.0000073824', 'Gbit/s');
      await page.save(user);
      expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
      expect(rawStore()).toBeNull();
    });

    it('the remembered value is what is checked: a value that only rounds up is refused after a unit change too', async () => {
      const user = await page.openFull();
      await typeConverted(user, '0.0073824'); // 0.6 pps
      await pickUnit(user, body(), 'Initialization rate', 'bit/s'); // stays in the rate family
      await page.save(user);
      expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
      expect(rawStore()).toBeNull();
    });

    it('a refused 0.6 pps can be corrected to a valid rate and saved', async () => {
      const user = await page.openFull();
      await typeConverted(user, '0.0073824');
      await page.save(user);
      await screen.findByRole('alert');
      await typeInto(user, body(), 'Initialization rate', '12.304', 'Gbit/s'); // 1e6 pps at 1518 B
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(1000000);
    });
  },
);
