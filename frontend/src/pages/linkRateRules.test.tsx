import { screen } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import { PAGES } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  PPS_UNITS,
  RATE_UNITS,
  expectOptional,
  expectRequired,
  numberField,
  pickUnit,
  rawStore,
  removeButton,
  storedLinks,
  typeInto,
  unitOf,
  unitSelect,
} from '../test/helpers';

// Spec 25 review follow-up (coordinator, 2026-11-04).
//
// 1. A unit of the other family can only be CONVERTED while the average frame size is valid. While it is empty or invalid
//    the unit select offers the own family plus ONLY the currently selected cross-family unit (not the whole other
//    family). Switching from that cross unit to an own-family unit cannot convert the number, so it CLEARS the number text.
//    The same holds for Initialization rate (pps family own, rate units cross) and Rate (rate units own, pps units cross).
// 3. The limiter Rate must be greater than 0 (the controller refuses it): 0 bit/s, or a pps value that rounds to 0 bit/s,
//    is refused on save with an error naming "Rate". (No existing test pins 0 as accepted: checked by grep.)

const body = () => document.body;
const FRAME = 'Average frame size';
const typeFrame = (user: UserEvent, text: string) => typeInto(user, body(), FRAME, text);
const optionsOf = (label: string) => Array.from(unitSelect(body(), label).options).map((o) => o.textContent);
const shown = (label: string) => numberField(body(), label).value;
const storedConfig = (_page: ConfigPage): ConfigDto => {
  expect(storedLinks()).toHaveLength(1);
  return storedLinks()[0].config;
};

describe.each(PAGES)('$name: a cross-family unit without a valid frame size', (page: ConfigPage) => {
  // [what is wrong with the frame size, how the test makes it so]
  const BAD_FRAMES: [string, string][] = [
    ['empty', ''],
    ['invalid (63 B)', '63'],
  ];

  describe.each(BAD_FRAMES)('frame size %s', (_name, bad) => {
    it('Initialization rate keeps only the selected cross unit: own family + that one', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeFrame(user, bad);
      expect(optionsOf('Initialization rate')).toEqual([...PPS_UNITS, 'Gbit/s']);
      expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
      expect(shown('Initialization rate')).toBe('100');
    });

    it('Rate keeps only the selected cross unit: own family + that one', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Rate', '8', 'Mpps');
      await typeFrame(user, bad);
      expect(optionsOf('Rate')).toEqual([...RATE_UNITS, 'Mpps']);
      expect(unitOf(body(), 'Rate')).toBe('Mpps');
      expect(shown('Rate')).toBe('8');
    });

    it.each([['Mbit/s'], ['bit/s']])(
      'Initialization rate selected in %s offers exactly pps|kpps|Mpps and that unit',
      async (cross) => {
        const user = await page.openFull();
        await typeFrame(user, '1518');
        await typeInto(user, body(), 'Initialization rate', '5000', cross);
        await typeFrame(user, bad);
        expect(optionsOf('Initialization rate')).toEqual([...PPS_UNITS, cross]);
      },
    );

    it.each([['kpps'], ['pps']])('Rate selected in %s offers exactly bit/s..Gbit/s and that unit', async (cross) => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Rate', '5000', cross);
      await typeFrame(user, bad);
      expect(optionsOf('Rate')).toEqual([...RATE_UNITS, cross]);
    });

    it('both rate fields in the other family: each keeps its own selected unit only', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeInto(user, body(), 'Rate', '8', 'Mpps');
      await typeFrame(user, bad);
      expect(optionsOf('Initialization rate')).toEqual([...PPS_UNITS, 'Gbit/s']);
      expect(optionsOf('Rate')).toEqual([...RATE_UNITS, 'Mpps']);
    });

    it('switching Initialization rate from the cross unit to an own unit clears the number (no reinterpretation)', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeFrame(user, bad);
      await pickUnit(user, body(), 'Initialization rate', 'Mpps');
      expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
      expect(shown('Initialization rate')).toBe(''); // never "100" Mpps
    });

    it.each(PPS_UNITS)('... also to %s', async (own) => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeFrame(user, bad);
      await pickUnit(user, body(), 'Initialization rate', own);
      expect(unitOf(body(), 'Initialization rate')).toBe(own);
      expect(shown('Initialization rate')).toBe('');
    });

    it('switching Rate from pps to an own unit clears the number', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Rate', '8', 'Mpps');
      await typeFrame(user, bad);
      await pickUnit(user, body(), 'Rate', 'Gbit/s');
      expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
      expect(shown('Rate')).toBe(''); // never "8" Gbit/s
    });

    it.each(RATE_UNITS)('... also to %s', async (own) => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Rate', '8', 'Mpps');
      await typeFrame(user, bad);
      await pickUnit(user, body(), 'Rate', own);
      expect(unitOf(body(), 'Rate')).toBe(own);
      expect(shown('Rate')).toBe('');
    });

    it('after the switch the cross unit is gone, the frame size is optional again and the field takes a new number', async () => {
      const user = await page.openFull();
      await page.ready(user);
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeFrame(user, bad);
      expectRequired(numberField(body(), FRAME));
      await pickUnit(user, body(), 'Initialization rate', 'Mpps');
      await typeFrame(user, ''); // an invalid (63 B) value would still be refused on save
      expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
      expectOptional(numberField(body(), FRAME));
      await typeInto(user, body(), 'Initialization rate', '5');
      await page.save(user);
      expect(storedConfig(page).delayer?.initialization_rate_pps).toBe(5000000);
    });

    it('switching one field leaves the other field alone', async () => {
      const user = await page.openFull();
      await typeFrame(user, '1518');
      await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
      await typeInto(user, body(), 'Rate', '8', 'Mpps');
      await typeFrame(user, bad);
      await pickUnit(user, body(), 'Initialization rate', 'Mpps');
      expect(shown('Initialization rate')).toBe('');
      expect(unitOf(body(), 'Rate')).toBe('Mpps');
      expect(shown('Rate')).toBe('8');
      expect(optionsOf('Rate')).toEqual([...RATE_UNITS, 'Mpps']);
    });
  });

  it('typing a valid frame size again offers the whole other family again and keeps the number', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await typeFrame(user, '');
    expect(optionsOf('Initialization rate')).toEqual([...PPS_UNITS, 'Gbit/s']);
    await typeFrame(user, '1518');
    expect(optionsOf('Initialization rate')).toEqual([...PPS_UNITS, ...RATE_UNITS]);
    expect(unitOf(body(), 'Initialization rate')).toBe('Gbit/s');
    expect(shown('Initialization rate')).toBe('100');
  });

  it('with a valid frame size the change still CONVERTS (the new rule does not touch it)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '100', 'Gbit/s');
    await pickUnit(user, body(), 'Initialization rate', 'Mpps');
    expect(shown('Initialization rate')).toBe('8.127');
    await typeInto(user, body(), 'Rate', '8', 'Mpps');
    await pickUnit(user, body(), 'Rate', 'Gbit/s');
    expect(shown('Rate')).toBe('98.432');
  });

  it('a field in its own family keeps its own-family options only, whatever the frame size says', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeFrame(user, '');
    expect(optionsOf('Initialization rate')).toEqual(PPS_UNITS);
    expect(optionsOf('Rate')).toEqual(RATE_UNITS);
  });

  it('an empty field with a cross unit selected is cleared too (nothing to convert, nothing to reinterpret)', async () => {
    const user = await page.openFull();
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Initialization rate', '', 'Gbit/s');
    await typeFrame(user, '');
    await pickUnit(user, body(), 'Initialization rate', 'kpps');
    expect(shown('Initialization rate')).toBe('');
    expect(unitOf(body(), 'Initialization rate')).toBe('kpps');
  });
});

describe.each(PAGES)('$name: the limiter Rate must be greater than 0', (page: ConfigPage) => {
  const refuses = async (user: UserEvent) => {
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/\bRate\b/);
    expect(page.requests()).toBe(0);
    expect(rawStore()).toBeNull();
  };

  it.each([
    ['0', 'bit/s'],
    ['0', 'kbit/s'],
    ['0', 'Mbit/s'],
    ['0', 'Gbit/s'],
    ['0.0', 'Gbit/s'],
  ])('Rate %s %s is refused with an error naming "Rate"; nothing is written', async (text, unit) => {
    const user = await page.openFull();
    await page.ready(user);
    await typeInto(user, body(), 'Rate', text, unit);
    await refuses(user);
  });

  it.each([
    ['0', 'pps'],
    ['0', 'Mpps'],
  ])('Rate %s %s (frame size set) is refused', async (text, unit) => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Rate', text, unit);
    await refuses(user);
  });

  it('a pps value that rounds to 0 bit/s is refused: 0.00001 pps at 1518 B is 0.12 bit/s', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Rate', '0.00001', 'pps');
    await refuses(user);
  });

  it('0.00003 pps at 64 B (0.0025 bit/s) is refused', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '64');
    await typeInto(user, body(), 'Rate', '0.00003', 'pps');
    await refuses(user);
  });

  it('the smallest value that rounds to 1 bit/s is accepted: 0.0001 pps at 1518 B (1.2304 bit/s) stores 1', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeFrame(user, '1518');
    await typeInto(user, body(), 'Rate', '0.0001', 'pps');
    await page.save(user);
    expect(storedConfig(page).limiter?.rate_bps).toBe(1);
  });

  it('1 bit/s is accepted and stored as 1', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeInto(user, body(), 'Rate', '1', 'bit/s');
    await page.save(user);
    expect(storedConfig(page).limiter?.rate_bps).toBe(1);
  });

  it('a refused 0 keeps the typed values and can be corrected and saved', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeInto(user, body(), 'Rate', '0', 'Gbit/s');
    await refuses(user);
    expect(shown('Rate')).toBe('0');
    expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
    await typeInto(user, body(), 'Rate', '2', 'Gbit/s');
    await page.save(user);
    expect(storedConfig(page).limiter?.rate_bps).toBe(2e9);
  });

  it('a rate of 0 with the limiter removed again does not matter: the limiter is sent as null / absent', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeInto(user, body(), 'Rate', '0', 'Gbit/s');
    await user.click(removeButton('limiter'));
    await page.save(user);
    expect(storedConfig(page).limiter ?? null).toBeNull();
  });

  it('a form without a limiter is not affected', async () => {
    const user = await page.openNew();
    await page.ready(user);
    await page.save(user);
    expect(storedConfig(page).limiter ?? null).toBeNull();
  });
});
