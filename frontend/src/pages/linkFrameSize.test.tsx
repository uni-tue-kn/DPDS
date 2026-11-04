import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto, Link, LocalConfig, Preset } from '../api/types';
import { startRequestOf } from '../common/localConfig';
import { pendingRestart } from '../common/pendingRestart';
import { api } from '../test/mockApi';
import * as fx from '../test/fixtures';
import { linkPage, presetPage, PAGES } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  PPS_UNITS,
  RATE_UNITS,
  allRequiredMarkers,
  expectOptional,
  expectRequired,
  expectStartBody,
  field,
  numberField,
  pickUnit,
  rawStore,
  renderApp,
  seedLinks,
  seedPresets,
  seedStore,
  storedDoc,
  storedLinks,
  storedOrEmpty,
  storedPresets,
  typeInto,
  unitOf,
  unitSelect,
  watchStoreWrites,
} from '../test/helpers';

// Spec 25 (user 2026-11-04): "Average frame size" is a per-link and per-preset value that lives ONLY in the browser's local
// document, as the top-level optional member `avg_frame_size_byte` of the link / preset (NOT inside `config`). It is the
// L2 Ethernet frame including FCS (e.g. 1518 B); conversion between packet rates and bit rates adds 20 B of L1 overhead:
//   pps = bit/s / ((frame + 20) x 8)        bit/s = pps x (frame + 20) x 8
// It has no default and is optional. While it is empty or invalid, "Initialization rate" offers only pps|kpps|Mpps and
// "Rate" only bit/s..Gbit/s; once it holds a valid value (finite, >= 64) both offer both families, own family first.
// Choices of the tester where the spec is silent are marked "tester's choice" below.

/** The optional top-level member the spec adds to links and presets (types.ts does not have it yet). */
interface Framed {
  avg_frame_size_byte?: number | null;
}
type FramedLink = Link & Framed;
type FramedPreset = Preset & Framed;

const withFrame = <T extends Link | Preset>(entry: T, frame: number | null): T & Framed => ({
  ...entry,
  avg_frame_size_byte: frame,
});

const body = () => document.body;
const isLink = (page: ConfigPage) => page.name === 'link editor';

const FRAME = 'Average frame size';
const frameInput = () => numberField(body(), FRAME);
const typeFrame = (user: UserEvent, text: string) => typeInto(user, body(), FRAME, text);
const optionsOf = (label: string) => Array.from(unitSelect(body(), label).options).map((o) => o.textContent);
const shown = (label: string) => numberField(body(), label).value;

const BOTH_FROM_PPS = [...PPS_UNITS, ...RATE_UNITS];
const BOTH_FROM_RATE = [...RATE_UNITS, ...PPS_UNITS];

/** The one stored link / preset after saving a new form, with its top-level frame size member. */
function storedEntry(page: ConfigPage): (FramedLink | FramedPreset) & { config: ConfigDto } {
  const list = isLink(page) ? storedLinks() : storedPresets();
  expect(list).toHaveLength(1);
  return list[0] as (FramedLink | FramedPreset) & { config: ConfigDto };
}
const storedFrame = (page: ConfigPage) => storedEntry(page).avg_frame_size_byte ?? null;

/** Opens the edit form of link / preset 1 whose config is `config` and whose stored frame size is `frame`. */
async function openStored(page: ConfigPage, frame: number | null, config: ConfigDto = fx.config()): Promise<UserEvent> {
  const user = userEvent.setup();
  if (isLink(page)) {
    seedLinks([withFrame(fx.link({ config }), frame)]);
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('a-b'));
  } else {
    seedPresets([withFrame(fx.preset({ id: 1, name: 'stored', config }), frame)]);
    renderApp('/presets/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit preset' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('stored'));
  }
  return user;
}

describe.each(PAGES)('$name: the Average frame size field', (page: ConfigPage) => {
  it('is one number input in the Buffer section after Seed, starting EMPTY (no default)', async () => {
    await page.openNew();
    const buffer = screen.getByRole('group', { name: 'Buffer' });
    const input = within(buffer).getByLabelText(FRAME) as HTMLInputElement;
    expect(input).toHaveAttribute('type', 'number');
    expect(input).toHaveValue(null);
    expect(screen.getAllByLabelText(FRAME)).toHaveLength(1);
    const seed = within(buffer).getByLabelText('Seed');
    expect(seed.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('has a static "B" addon and no unit select', async () => {
    await page.openNew();
    const group = frameInput().closest('.input-group') as HTMLElement | null;
    expect(group).not.toBeNull();
    const addon = (group as HTMLElement).querySelector('.input-group-text');
    expect(addon?.textContent?.trim()).toBe('B');
    expect(screen.queryByLabelText(`${FRAME} unit`)).not.toBeInTheDocument();
    expect((group as HTMLElement).querySelector('select')).toBeNull();
  });

  it('is optional: no marker, not required, and it does not add a required marker to the form', async () => {
    const user = await page.openFull();
    expectOptional(frameInput());
    const before = allRequiredMarkers().length;
    await typeFrame(user, '1518');
    expectOptional(frameInput());
    expect(allRequiredMarkers()).toHaveLength(before);
  });

  it('a new form saved without it stores no frame size, nothing else changes', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await page.save(user);
    expect(storedFrame(page)).toBeNull();
    expect(JSON.stringify(storedEntry(page).config)).not.toMatch(/frame/i);
  });

  it('a valid frame size alone is stored top-level in bytes, not inside config', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await page.save(user);
    expect(storedFrame(page)).toBe(1518);
    expect(JSON.stringify(storedEntry(page).config)).not.toMatch(/frame/i);
  });

  it.each([
    ['the minimum 64', '64', 64],
    ['a decimal (it is an average)', '1518.5', 1518.5],
    ['a jumbo frame', '9000', 9000],
  ])('accepts %s', async (_name, text, want) => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, text);
    await page.save(user);
    expect(storedFrame(page)).toBe(want);
  });

  it.each([
    ['below the Ethernet minimum', '63'],
    ['just below 64', '63.99'],
    ['zero', '0'],
    ['negative', '-5'],
  ])('refuses %s ("%s"): an error names "Average frame size", nothing is written', async (_name, text) => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, text);
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(page.requests()).toBe(0);
    expect(rawStore()).toBeNull();
  });

  it('an invalid value is refused even when no rate field uses it', async () => {
    const user = await page.openNew();
    await page.ready(user);
    await typeFrame(user, '10');
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(page.requests()).toBe(0);
  });
});

describe.each(PAGES)('$name: editing a stored entry', (page: ConfigPage) => {
  it('shows the stored frame size and keeps it (and the config) through an unchanged save', async () => {
    const user = await openStored(page, 9000);
    expect(frameInput()).toHaveValue(9000);
    await page.save(user);
    await waitFor(() => expect(storedFrame(page)).toBe(9000));
    expect(storedEntry(page).config).toEqual(fx.config());
  });

  it('a stored entry without a frame size opens with an empty field and only the own-family units', async () => {
    await openStored(page, null);
    expect(frameInput()).toHaveValue(null);
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
  });

  it('with a stored frame size the rate fields offer both families, shown in their own family', async () => {
    await openStored(page, 1518, fx.config({ limiter: { mode: 'Spacer', rate_bps: 5e9 } }));
    expect(optionsOf('Initialization rate')).toEqual(BOTH_FROM_PPS);
    expect(optionsOf('Rate')).toEqual(BOTH_FROM_RATE);
    expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
    expect(shown('Initialization rate')).toBe('12.5');
    expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
    expect(shown('Rate')).toBe('5');
  });

  it('the frame size can be changed', async () => {
    const user = await openStored(page, 9000);
    await typeFrame(user, '1500');
    await page.save(user);
    await waitFor(() => expect(storedFrame(page)).toBe(1500));
  });

  it('the frame size can be cleared: it is then stored as null or absent', async () => {
    const user = await openStored(page, 9000);
    await typeFrame(user, '');
    await page.save(user);
    await waitFor(() => expect(storedFrame(page)).toBeNull());
  });

  it('an unrelated edit keeps the stored frame size', async () => {
    const user = await openStored(page, 1518);
    await typeInto(user, body(), 'Buffer capacity', '4321');
    await page.save(user);
    await waitFor(() => expect(storedEntry(page).config.buffer_capacity).toBe(4321));
    expect(storedFrame(page)).toBe(1518);
  });
});

describe.each(PAGES)('$name: unit options depend on the frame size', (page: ConfigPage) => {
  it('without a frame size: Initialization rate has only pps|kpps|Mpps, Rate only bit/s..Gbit/s', async () => {
    await page.openFull();
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
    expect(optionsOf('Rate')).toEqual(RATE_UNITS);
  });

  it('with a valid frame size both fields offer both families, own family first', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    expect(optionsOf('Initialization rate')).toEqual(BOTH_FROM_PPS);
    expect(optionsOf('Rate')).toEqual(BOTH_FROM_RATE);
  });

  it('the default units stay Mpps and Gbit/s', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
    expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
  });

  it.each([
    ['64 (the minimum)', '64'],
    ['a decimal', '1518.5'],
  ])('%s is valid: both families are offered', async (_n, text) => {
    const user = await page.openFull();
    await typeFrame(user, text);
    expect(optionsOf('Initialization rate')).toEqual(BOTH_FROM_PPS);
    expect(optionsOf('Rate')).toEqual(BOTH_FROM_RATE);
  });

  it.each([
    ['63', '63'],
    ['0', '0'],
    ['-100', '-100'],
  ])('an invalid frame size (%s) offers only the own family', async (_n, text) => {
    const user = await page.openFull();
    await typeFrame(user, text);
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
    expect(optionsOf('Rate')).toEqual(RATE_UNITS);
  });

  it('clearing a valid frame size takes the other family away again (nothing of the other family is selected)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    expect(optionsOf('Rate')).toEqual(BOTH_FROM_RATE);
    await typeFrame(user, '');
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
    expect(optionsOf('Rate')).toEqual(RATE_UNITS);
  });

  it('the other units belong to the same select (still one unit select per rate field)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    expect(screen.getAllByLabelText('Initialization rate unit')).toHaveLength(1);
    expect(screen.getAllByLabelText('Rate unit')).toHaveLength(1);
  });
});

// [field, typed value, from unit, to unit, shown value after the change]
const CROSS: [string, string, string, string, string][] = [
  ['Initialization rate', '100', 'Gbit/s', 'Mpps', '8.127'], // 1e11 / 12304 = 8127438.23 pps (spec example)
  ['Initialization rate', '100', 'Gbit/s', 'kpps', '8127.438'],
  ['Initialization rate', '100', 'Gbit/s', 'pps', '8127438.231'],
  ['Initialization rate', '12.304', 'Gbit/s', 'Mpps', '1'], // exactly 1e6 pps: trailing zeros are dropped
  ['Initialization rate', '8', 'Mpps', 'Gbit/s', '98.432'], // 8e6 x 12304 = 98.432e9 bit/s (spec example)
  ['Initialization rate', '8', 'Mpps', 'Mbit/s', '98432'],
  ['Initialization rate', '8', 'Mpps', 'bit/s', '98432000000'],
  ['Initialization rate', '8.127', 'Mpps', 'Gbit/s', '99.995'], // 99.994608 rounded to 3 decimals
  ['Initialization rate', '1500', 'kpps', 'Gbit/s', '18.456'],
  ['Rate', '8', 'Mpps', 'Gbit/s', '98.432'],
  ['Rate', '100', 'Gbit/s', 'Mpps', '8.127'],
  ['Rate', '100', 'Gbit/s', 'kpps', '8127.438'],
  ['Rate', '12.304', 'Gbit/s', 'Mpps', '1'],
  ['Rate', '98432', 'Mbit/s', 'Mpps', '8'],
];

describe.each(PAGES)('$name: changing the unit across families converts the shown number', (page: ConfigPage) => {
  it.each(CROSS)('%s: %s %s -> %s shows %s (frame 1518 B)', async (label, text, from, to, want) => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), label, text, from);
    await pickUnit(user, body(), label, to);
    expect(unitOf(body(), label)).toBe(to);
    expect(shown(label)).toBe(want);
  });

  it('uses the entered frame size: 100 Gbit/s at 64 B is 148.81 Mpps', async () => {
    const user = await page.openFull();
    await typeFrame(user, '64');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('148.81');
  });

  it('5 Mpps at 64 B is 3.36 Gbit/s', async () => {
    const user = await page.openFull();
    await typeFrame(user, '64');
    await typeInto(user, body(), 'Rate', '5', 'Mpps');
    await pickUnit(user, body(), 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('3.36');
  });

  it('an empty field stays empty when the unit changes across families', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '', 'Gbit/s');
    expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
    expect(shown('Initialization rate')).toBe('');
  });

  it('within a family the change is still decimal-exact (frame size set)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '2.5', 'Mpps');
    await pickUnit(user, body(), 'Initialization rate', 'kpps');
    expect(shown('Initialization rate')).toBe('2500');
    await pickUnit(user, body(), 'Initialization rate', 'pps');
    expect(shown('Initialization rate')).toBe('2500000');
    await typeInto(user, body(), 'Rate', '5', 'Gbit/s');
    await pickUnit(user, body(), 'Rate', 'Mbit/s');
    expect(shown('Rate')).toBe('5000');
    await pickUnit(user, body(), 'Rate', 'kbit/s');
    expect(shown('Rate')).toBe('5000000');
  });

  it('within a family the change is exact without a frame size too', async () => {
    const user = await page.openFull();
    await typeInto(user, body(), 'Initialization rate', '8.2', 'Mpps');
    await pickUnit(user, body(), 'Initialization rate', 'kpps');
    expect(shown('Initialization rate')).toBe('8200');
  });
});

describe.each(PAGES)('$name: saving a value typed in the other family', (page: ConfigPage) => {
  /** Frame size and the unit/value of one rate field, set after the form is otherwise ready. */
  async function fill(user: UserEvent, frame: string, label: string, text: string, unit: string) {
    await page.ready(user);
    await typeFrame(user, frame);
    await typeInto(user, body(), label, text, unit);
  }

  it('Initialization rate 100 Gbit/s at 1518 B is stored as 8127438 pps (rounded to a whole pps)', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Initialization rate', '100', 'Gbit/s');
    await page.save(user);
    const c = storedEntry(page).config;
    expect(c.delayer?.initialization_rate_pps).toBe(8127438);
    expect(storedFrame(page)).toBe(1518);
  });

  it('rounds to the NEAREST pps, not down: 1.5 Gbit/s at 1518 B is 121912 pps', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Initialization rate', '1.5', 'Gbit/s');
    await page.save(user);
    expect(storedEntry(page).config.delayer?.initialization_rate_pps).toBe(121912); // 121911.57...
  });

  it('Limiter rate 8 Mpps at 1518 B is stored as 98432000000 bit/s', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Rate', '8', 'Mpps');
    await page.save(user);
    const c = storedEntry(page).config;
    expect(c.limiter?.rate_bps).toBe(98432000000);
    expect(c.delayer?.initialization_rate_pps).toBe(10000000); // the helper's own rate is untouched
    expect(storedFrame(page)).toBe(1518);
  });

  it('rounds the bit rate to a whole bit/s: 1 pps at 100.1 B is 961 bit/s (960.8)', async () => {
    const user = await page.openFull();
    await fill(user, '100.1', 'Rate', '1', 'pps');
    await page.save(user);
    expect(storedEntry(page).config.limiter?.rate_bps).toBe(961);
    expect(storedFrame(page)).toBe(100.1);
  });

  it('both fields in the other family at once', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Rate', '8', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await page.save(user);
    const c = storedEntry(page).config;
    expect(c.limiter?.rate_bps).toBe(98432000000);
    expect(c.delayer?.initialization_rate_pps).toBe(8127438);
  });

  it('a value in the own family is stored unchanged even with a frame size', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Initialization rate', '5', 'Mpps');
    await typeInto(user, body(), 'Rate', '5', 'Gbit/s');
    await page.save(user);
    const c = storedEntry(page).config;
    expect(c.delayer?.initialization_rate_pps).toBe(5000000);
    expect(c.limiter?.rate_bps).toBe(5e9);
  });

  it('the stored values are whole numbers', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Rate', '3.3', 'Mpps');
    await typeInto(user, body(), 'Initialization rate', '7.7', 'Gbit/s');
    await page.save(user);
    const c = storedEntry(page).config;
    expect(Number.isInteger(c.limiter?.rate_bps)).toBe(true);
    expect(Number.isInteger(c.delayer?.initialization_rate_pps)).toBe(true);
  });

  it('the >= 1 pps minimum applies after conversion: 5000 bit/s at 1518 B (0.41 pps) is refused', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Initialization rate', '0.000005', 'Gbit/s');
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(page.requests()).toBe(0);
    expect(rawStore()).toBeNull();
  });

  it('the converted value is accepted when it is at least 1 pps: 0.1 Gbit/s at 1518 B', async () => {
    const user = await page.openFull();
    await fill(user, '1518', 'Initialization rate', '0.1', 'Gbit/s');
    await page.save(user);
    expect(storedEntry(page).config.delayer?.initialization_rate_pps).toBe(8127); // 8127.44
  });
});

describe.each(PAGES)('$name: the frame size is conditionally required', (page: ConfigPage) => {
  it('is marked required exactly while a rate field shows a unit of the other family', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    expectOptional(frameInput());
    const before = allRequiredMarkers().length;

    await pickUnit(user, body(), 'Initialization rate', 'Gbit/s');
    expectRequired(frameInput());
    expect(allRequiredMarkers()).toHaveLength(before + 1);

    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expectOptional(frameInput());
    expect(allRequiredMarkers()).toHaveLength(before);

    await pickUnit(user, body(), 'Rate', 'Mpps');
    expectRequired(frameInput());
    expect(allRequiredMarkers()).toHaveLength(before + 1);

    // both fields cross-family: still one marker
    await pickUnit(user, body(), 'Initialization rate', 'Gbit/s');
    expectRequired(frameInput());
    expect(allRequiredMarkers()).toHaveLength(before + 1);

    await pickUnit(user, body(), 'Rate', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expectOptional(frameInput());
  });

  it('a form that only has the own-family units never marks it', async () => {
    await page.openFull();
    expectOptional(frameInput());
  });

  it('clearing the frame size keeps the cross-family unit visible and selected, marked required', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeFrame(user, '');
    expect(optionsOf('Initialization rate')).toContain('Gbit/s');
    expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
    expect(optionsOf('Initialization rate')).toEqual(expect.arrayContaining(PPS_UNITS));
    expectRequired(frameInput());
  });

  it('saving then is refused with an error naming "Average frame size"; nothing is written', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeFrame(user, '');
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(page.requests()).toBe(0);
    expect(rawStore()).toBeNull();
  });

  it('the same for the limiter rate', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Rate', '8', 'Mpps');
    await typeFrame(user, '');
    expect(unitOf(body(), 'Rate')).toBe('Mpps');
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(page.requests()).toBe(0);
  });

  it('an invalid (< 64) frame size with a cross-family unit selected keeps the unit and refuses the save', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeFrame(user, '63');
    expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(page.requests()).toBe(0);
  });

  it('going back to an own-family unit and retyping lets the save through without a frame size', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeFrame(user, '');
    await typeInto(user, body(), 'Initialization rate', '5', 'Mpps');
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
    expectOptional(frameInput());
    await page.save(user);
    expect(storedEntry(page).config.delayer?.initialization_rate_pps).toBe(5000000);
    expect(storedFrame(page)).toBeNull();
  });
});

describe('Link editor: Apply preset and Save as preset carry the frame size', () => {
  const seededPresets = () => [
    withFrame(fx.preset({ id: 1, name: 'jumbo', config: fx.config3() }), 9000),
    withFrame(fx.preset({ id: 2, name: 'plain', config: fx.config2() }), null),
  ];
  const apply = (user: UserEvent, name: string) => user.selectOptions(screen.getByLabelText('Apply preset'), name);

  it("copies the preset's frame size into the link form", async () => {
    seedPresets(seededPresets());
    const user = await linkPage.openNew();
    expect(frameInput()).toHaveValue(null);
    await apply(user, 'jumbo');
    expect(frameInput()).toHaveValue(9000);
    expect(optionsOf('Initialization rate')).toEqual(BOTH_FROM_PPS);
  });

  it('replaces a typed frame size', async () => {
    seedPresets(seededPresets());
    const user = await linkPage.openNew();
    await typeFrame(user, '1518');
    await apply(user, 'jumbo');
    expect(frameInput()).toHaveValue(9000);
  });

  it('a preset without a frame size CLEARS the form field', async () => {
    seedPresets(seededPresets());
    const user = await linkPage.openNew();
    await typeFrame(user, '1518');
    await apply(user, 'plain');
    expect(frameInput()).toHaveValue(null);
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
  });

  it('an applied frame size is saved with the link', async () => {
    seedPresets(seededPresets());
    const user = await linkPage.openNew();
    await apply(user, 'jumbo');
    await linkPage.ready(user);
    await linkPage.save(user);
    await waitFor(() => expect(storedLinks()).toHaveLength(1));
    expect((storedLinks()[0] as FramedLink).avg_frame_size_byte).toBe(9000);
  });

  it('is not stored in the preset by applying it (the link is a copy)', async () => {
    seedPresets(seededPresets());
    const user = await linkPage.openNew();
    await apply(user, 'jumbo');
    await typeFrame(user, '1500');
    await linkPage.ready(user);
    await linkPage.save(user);
    await waitFor(() => expect(storedLinks()).toHaveLength(1));
    expect((storedPresets()[0] as FramedPreset).avg_frame_size_byte).toBe(9000);
    expect((storedLinks()[0] as FramedLink).avg_frame_size_byte).toBe(1500);
  });

  async function saveAsPreset(user: UserEvent, name: string): Promise<HTMLElement> {
    await user.click(screen.getByRole('button', { name: 'Save as preset' }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('Preset name'), name);
    await user.click(within(dialog).getByRole('button', { name: 'Save preset' }));
    return dialog;
  }

  it("Save as preset copies the link's frame size", async () => {
    const user = await linkPage.openNew();
    await typeFrame(user, '1518');
    await saveAsPreset(user, 'from-link');
    await waitFor(() => expect(storedPresets()).toHaveLength(1));
    const p = storedPresets()[0] as FramedPreset;
    expect(p.avg_frame_size_byte).toBe(1518);
    expect(JSON.stringify(p.config)).not.toMatch(/frame/i);
  });

  it('Save as preset without a frame size stores none', async () => {
    const user = await linkPage.openNew();
    await saveAsPreset(user, 'from-link');
    await waitFor(() => expect(storedPresets()).toHaveLength(1));
    expect((storedPresets()[0] as FramedPreset).avg_frame_size_byte ?? null).toBeNull();
  });

  it('Save as preset stores converted base values together with the frame size', async () => {
    const user = await linkPage.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeInto(user, body(), 'Rate', '8', 'Mpps');
    await saveAsPreset(user, 'cross');
    await waitFor(() => expect(storedPresets()).toHaveLength(1));
    const p = storedPresets()[0] as FramedPreset;
    expect(p.avg_frame_size_byte).toBe(1518);
    expect(p.config.delayer?.initialization_rate_pps).toBe(8127438);
    expect(p.config.limiter?.rate_bps).toBe(98432000000);
  });

  // tester's choice: the dialog validates the whole form like Save does, so a bad frame size is refused there too
  it('Save as preset with an invalid frame size is refused with an error naming "Average frame size"', async () => {
    const user = await linkPage.openNew();
    await typeFrame(user, '63');
    const dialog = await saveAsPreset(user, 'bad');
    expect(await within(dialog).findByRole('alert')).toHaveTextContent(/Average frame size/);
    expect(storedOrEmpty().presets).toHaveLength(0);
  });

  it('the preset editor stores its own frame size on the preset', async () => {
    const user = await presetPage.openNew();
    await presetPage.ready(user);
    await typeFrame(user, '9000');
    await presetPage.save(user);
    expect((storedPresets()[0] as FramedPreset).avg_frame_size_byte).toBe(9000);
  });
});

describe('the frame size never reaches the controller and never marks a restart', () => {
  const frameLink = () => withFrame(fx.link(), 1518);

  it('startRequestOf leaves it out of the start request', () => {
    const doc: LocalConfig = {
      version: 1,
      links: [frameLink(), withFrame(fx.linkRev(), 9000)],
      presets: [withFrame(fx.preset(), 64)],
      settings: fx.settings(),
    };
    const req = startRequestOf(doc);
    expect(JSON.stringify(req)).not.toMatch(/frame/i);
    for (const l of req.links) expect(Object.keys(l).sort()).toEqual(['config', 'egress', 'ingress', 'name', 'worker']);
  });

  it('Start sends the document without the frame size', async () => {
    seedStore({ links: [frameLink()] });
    const user = userEvent.setup();
    renderApp('/');
    const btn = await screen.findByRole('button', { name: /^start$/i });
    await waitFor(() => expect(btn).toBeEnabled());
    await user.click(btn);
    expect(api.startEmulation).toHaveBeenCalledTimes(1);
    const sent = api.startEmulation.mock.calls[0][0];
    expectStartBody(sent, storedDoc());
    expect(JSON.stringify(sent)).not.toMatch(/frame/i);
  });

  it('the host hints POST body does not carry it either', async () => {
    seedStore({ links: [frameLink()] });
    renderApp('/hints');
    await waitFor(() => expect(api.getHostHintsFor).toHaveBeenCalledTimes(1));
    expect(JSON.stringify(api.getHostHintsFor.mock.calls[0][0])).not.toMatch(/frame/i);
  });

  it('a local link with a frame size equals the running link without one: no pending restart (pure)', () => {
    const local: LocalConfig = { version: 1, links: [frameLink()], presets: [], settings: fx.settings() };
    const running = fx.runningEmulation(fx.runningConfig([fx.link()], fx.settings()));
    expect(pendingRestart(local, running).any).toBe(false);
  });

  it('a different frame size alone does not mark a restart (pure)', () => {
    const running = fx.runningEmulation(fx.runningConfig([withFrame(fx.link(), 9000)], fx.settings()));
    const local: LocalConfig = { version: 1, links: [frameLink()], presets: [], settings: fx.settings() };
    expect(pendingRestart(local, running).any).toBe(false);
  });

  it('the Links page shows no pending-restart marker for it, but still for a real difference', async () => {
    const running = fx.runningEmulation(fx.runningConfig([fx.link()], fx.settings()));
    api.getEmulation.mockResolvedValue(running);
    seedStore({ links: [frameLink()], settings: fx.settings() });
    renderApp('/links');
    await screen.findByText('a-b');
    await waitFor(() => expect(api.getEmulation).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId('pending-restart')).not.toBeInTheDocument();
    expect(screen.queryByTestId('link-pending-restart')).not.toBeInTheDocument();
  });

  it('a real difference next to a frame size is still flagged (guards the two tests above)', async () => {
    const l = frameLink();
    const changed = { ...l, config: { ...l.config, buffer_capacity: l.config.buffer_capacity + 1 } };
    api.getEmulation.mockResolvedValue(fx.runningEmulation(fx.runningConfig([fx.link()], fx.settings())));
    seedStore({ links: [changed], settings: fx.settings() });
    renderApp('/links');
    expect(await screen.findByTestId('pending-restart')).toBeInTheDocument();
  });

  it('saving a link through the editor does not make the dashboard report a pending restart', async () => {
    api.getEmulation.mockResolvedValue(fx.runningEmulation(fx.runningConfig([fx.link()], fx.settings())));
    seedStore({ links: [frameLink()], settings: fx.settings() });
    const user = userEvent.setup();
    renderApp('/links/1');
    await screen.findByRole('heading', { level: 1, name: 'Edit link' });
    await waitFor(() => expect(screen.getByLabelText('Name')).toHaveValue('a-b'));
    await typeFrame(user, '9000');
    await user.click(screen.getByRole('button', { name: /^save$/i }));
    await screen.findByRole('heading', { level: 1, name: /links/i });
    await new Promise((r) => setTimeout(r, 30));
    expect((storedLinks()[0] as FramedLink).avg_frame_size_byte).toBe(9000);
    expect(screen.queryByTestId('pending-restart')).not.toBeInTheDocument();
  });
});

describe('Links: the inspect panel shows the frame size', () => {
  async function inspect(link: Link) {
    seedLinks([link]);
    const user = userEvent.setup();
    renderApp('/links');
    await screen.findByText(link.name);
    await user.click(screen.getByRole('button', { name: `Inspect ${link.name}` }));
    return screen.findByRole('dialog', { name: link.name });
  }

  it('has an "Average frame size" row "1518 B" in the Buffer group when set', async () => {
    const dialog = await inspect(withFrame(fx.link(), 1518));
    const buffer = within(dialog).getByRole('group', { name: 'Buffer' });
    expect(field(buffer, FRAME).textContent?.trim()).toBe('1518 B');
  });

  it('keeps decimals as stored', async () => {
    const dialog = await inspect(withFrame(fx.link(), 1518.5));
    expect(field(within(dialog).getByRole('group', { name: 'Buffer' }), FRAME).textContent?.trim()).toBe('1518.5 B');
  });

  it.each([null, undefined])('has no such row when it is %s', async (frame) => {
    const dialog = await inspect({ ...fx.link(), avg_frame_size_byte: frame } as Link);
    expect(within(dialog).queryByText(FRAME)).not.toBeInTheDocument();
    expect(dialog.textContent).not.toMatch(/frame size/i);
  });

  it('the other Buffer rows are unchanged', async () => {
    const dialog = await inspect(withFrame(fx.link(), 1518));
    const dts = Array.from(within(dialog).getByRole('group', { name: 'Buffer' }).querySelectorAll('dt')).map((d) =>
      d.textContent?.trim(),
    );
    expect(dts).toEqual(expect.arrayContaining(['Buffer capacity', 'Reordering', 'Seed']));
  });

  it('the Links list columns do not change (no frame size column)', async () => {
    seedLinks([withFrame(fx.link(), 1518)]);
    renderApp('/links');
    await screen.findByText('a-b');
    expect(screen.queryByRole('columnheader', { name: /frame/i })).not.toBeInTheDocument();
    expect(screen.getAllByRole('columnheader')).toHaveLength(
      within(screen.getAllByRole('row')[0]).getAllByRole('columnheader').length,
    );
  });
});

describe('the local document round trip keeps the frame size', () => {
  it('a link saved through the editor is written with the member at the link level', async () => {
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await typeFrame(user, '1518');
    await linkPage.save(user);
    const raw = JSON.parse(rawStore() ?? 'null') as LocalConfig;
    expect(Object.keys(raw.links[0]).sort()).toEqual(
      expect.arrayContaining(['avg_frame_size_byte', 'config', 'egress', 'id', 'ingress', 'name']),
    );
    expect(Object.keys(raw.links[0].config)).not.toContain('avg_frame_size_byte');
  });

  it('the write counter sees exactly one write for a save (the frame size adds no second one)', async () => {
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await typeFrame(user, '1518');
    const writes = watchStoreWrites();
    await linkPage.save(user);
    expect(writes()).toBe(1);
  });
});
