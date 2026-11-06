import { describe, expect, it } from 'vitest';
import type { ConfigDto } from '../api/types';
import {
  PACKET_UNITS,
  PPS_UNITS,
  RATE_UNITS,
  SIZE_UNITS,
  TIME_UNITS,
  changeUnit,
  formatUnit,
  fromBase,
  parseFrame,
  shiftDecimal,
  toBase,
} from '../common/units';
import type { Unit, UnitValue } from '../common/units';
import { dropNulls } from '../test/dropNulls';
import * as fx from '../test/fixtures';
import { must } from '../test/must';
import {
  GBIT,
  INIT_RATE_UNITS,
  LIMITER_RATE_UNITS,
  MPPS,
  MS,
  PACKETS,
  accuracyOf,
  configToForm,
  defaultConfigForm,
  formToDto,
  frameRequired,
  halfLifeRequired,
  linkProblem,
  rangeAdvice,
  portError,
  reorderingActive,
  stdRequired,
  withAccuracy,
} from './linkForm';
import type { ConfigForm } from './linkForm';
import { derivedLimitText } from './linkSummary';

// Pure logic of the config form: unit conversion, `formToDto` validation and the stored-value round trip. Nothing is rendered.

const MU = 'µs';
const at = (units: Unit[], label: string) => units.findIndex((u) => u.label === label);
const uv = (text: string, unit: number): UnitValue => ({ text, unit });
const form = (over: Partial<ConfigForm> = {}): ConfigForm => ({ ...defaultConfigForm(), ...over });
/** All three stages on, with a valid initialization rate (10 Mpps) which a new form does not have. */
const full = (over: Partial<ConfigForm> = {}): ConfigForm =>
  form({
    delayer_added: true,
    limiter_added: true,
    dropper_added: true,
    initialization_rate_pps: uv('10', MPPS),
    ...over,
  });
const cfgOf = (f: ConfigForm): ConfigDto => formToDto(f).config;
/** The histogram defaults of P4TG, in the wire form every saved link carries. */
const DELAY_DEFAULT = { bins: 16, lo_ns: 1024, hi_ns: 2048 };
const IAT_DEFAULT = { bins: 16, lo_ns: 0, hi_ns: 1024 };
const emptyHistogramFields = {
  hist_bins: '',
  hist_lo: uv('', 2),
  hist_hi: uv('', 2),
  iat_bins: '',
  iat_lo: uv('', 2),
  iat_hi: uv('', 2),
};
/** The message of the refusal; fails when the form is accepted. */
function refusal(f: ConfigForm): string {
  try {
    formToDto(f);
  } catch (e) {
    return (e as Error).message;
  }
  throw new Error('the form was accepted');
}
/** A refusal names the field and never a unit. */
const NO_UNIT_IN_MESSAGE = /\((ns|us|µs|ms|s|bit\/s|bit|pps)\)/;
function expectRefused(f: ConfigForm, field: RegExp): void {
  const m = refusal(f);
  expect(m).toMatch(field);
  expect(m).not.toMatch(NO_UNIT_IN_MESSAGE);
}

describe('units: labels and display of a stored value', () => {
  it('keeps the exact labels (micro sign U+00B5, never "us")', () => {
    expect(TIME_UNITS.map((u) => u.label)).toEqual(['ns', MU, 'ms', 's']);
    expect(RATE_UNITS.map((u) => u.label)).toEqual(['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s']);
    expect(PPS_UNITS.map((u) => u.label)).toEqual(['pps', 'kpps', 'Mpps']);
    expect(SIZE_UNITS.map((u) => u.label)).toEqual(['bit', 'kbit', 'Mbit', 'Gbit']);
    expect(PACKET_UNITS.map((u) => u.label)).toEqual(['packets', 'kpackets', 'Mpackets']);
    expect(TIME_UNITS.map((u) => u.exp)).toEqual([0, 3, 6, 9]);
  });

  it.each([
    ['2.5', 3, '2500'],
    ['2500', -3, '2.5'],
    ['1.005', 6, '1005000'],
    ['0.067', 9, '67000000'], // 0.067 * 1e9 is 67000000.00000001 in floating point
    ['-5', 3, '-5000'],
    ['-0', 3, '0'],
    ['.5', 1, '5'],
    ['1e3', 0, '1000'],
    ['0.0025', -6, '0.0000000025'],
  ])('shiftDecimal(%s, %d) = %s, exactly', (text, by, want) => {
    expect(shiftDecimal(text, by)).toBe(want);
  });

  it.each(['', ' ', 'abc', '.', '1.2.3'])('shiftDecimal of %j is null', (text) => {
    expect(shiftDecimal(text, 3)).toBeNull();
  });

  // [stored base value, shown "<number> <unit>"]: the largest unit in which the value is >= 1 with at most 3 decimals
  it.each([
    [0, '0 ms'],
    [1, '1 ns'],
    [999, '999 ns'],
    [1000, `1 ${MU}`],
    [2500, `2.5 ${MU}`],
    [1001, `1.001 ${MU}`],
    [1000500, `1000.5 ${MU}`],
    [999999999, `999999.999 ${MU}`],
    [1005000, '1.005 ms'],
    [8200000, '8.2 ms'],
    [1500000000, '1.5 s'],
    [0.0000001, '0.0000001 ns'],
  ])('time: %d ns is shown as %s', (ns, shown) => {
    expect(formatUnit(ns, TIME_UNITS, MS)).toBe(shown);
  });

  it.each([
    [999, '999 bit/s'],
    [1500, '1.5 kbit/s'],
    [1234567000, '1234.567 Mbit/s'],
    [40000000000, '40 Gbit/s'],
    [0, '0 Gbit/s'],
  ])('rate: %d bit/s is shown as %s', (bps, shown) => {
    expect(formatUnit(bps, RATE_UNITS, GBIT)).toBe(shown);
  });

  it.each([
    [1.5, '1.5 pps'],
    [6543210, '6543.21 kpps'],
    [12500000, '12.5 Mpps'],
    [0, '0 Mpps'],
  ])('pps: %d is shown as %s', (pps, shown) => {
    expect(formatUnit(pps, PPS_UNITS, MPPS)).toBe(shown);
  });

  it.each([
    [0, '0 packets'],
    [999, '999 packets'],
    [1001, '1.001 kpackets'],
    [100000, '100 kpackets'],
    [2000000, '2 Mpackets'],
  ])('packets: %d is shown as %s', (n, shown) => {
    expect(formatUnit(n, PACKET_UNITS, PACKETS)).toBe(shown);
  });

  it('every shown value converts back to exactly the stored one', () => {
    for (const ns of [1, 999, 1001, 12345, 1000001, 1005000, 999999999, 1500000000]) {
      expect(toBase(fromBase(ns, TIME_UNITS, MS), TIME_UNITS)).toBe(ns);
    }
    for (const bps of [999, 1500, 2500000, 1234567000, 40000000000]) {
      expect(toBase(fromBase(bps, RATE_UNITS, GBIT), RATE_UNITS)).toBe(bps);
    }
    expect(toBase(fromBase(1.5, PPS_UNITS, MPPS), PPS_UNITS)).toBe(1.5);
    expect(toBase(uv('', 0), TIME_UNITS)).toBeNull();
    expect(toBase(uv('abc', 0), TIME_UNITS)).toBeNull();
  });
});

describe('units: changing the unit of a field', () => {
  it('within a family the shown number is shifted exactly and the chain returns to the start', () => {
    let v = uv('2500', at(TIME_UNITS, 'ns'));
    const seen: string[] = [];
    for (const to of [MU, 'ms', 's', 'ns']) {
      v = changeUnit(v, TIME_UNITS, at(TIME_UNITS, to));
      seen.push(v.text);
    }
    expect(seen).toEqual(['2.5', '0.0025', '0.0000025', '2500']);
  });

  it('an empty or unparsable text stays as it is', () => {
    expect(changeUnit(uv('', 2), TIME_UNITS, 0)).toEqual(uv('', 0));
    expect(changeUnit(uv('abc', 2), TIME_UNITS, 0)).toEqual(uv('abc', 0));
  });

  // [field units, typed text, from, to, frame bytes, shown after the change]: pps = bit/s / ((frame + 20) x 8)
  const CROSS: [Unit[], string, string, string, number, string][] = [
    [INIT_RATE_UNITS, '100', 'Gbit/s', 'Mpps', 1518, '8.127'], // 1e11 / 12304 = 8127438.23 pps
    [INIT_RATE_UNITS, '100', 'Gbit/s', 'kpps', 1518, '8127.438'],
    [INIT_RATE_UNITS, '100', 'Gbit/s', 'pps', 1518, '8127438.231'],
    [INIT_RATE_UNITS, '12.304', 'Gbit/s', 'Mpps', 1518, '1'],
    [INIT_RATE_UNITS, '8', 'Mpps', 'Gbit/s', 1518, '98.432'],
    [INIT_RATE_UNITS, '8', 'Mpps', 'bit/s', 1518, '98432000000'],
    [INIT_RATE_UNITS, '1500', 'kpps', 'Gbit/s', 1518, '18.456'],
    [LIMITER_RATE_UNITS, '98432', 'Mbit/s', 'Mpps', 1518, '8'],
    [LIMITER_RATE_UNITS, '100', 'Gbit/s', 'Mpps', 64, '148.81'],
    [LIMITER_RATE_UNITS, '5', 'Mpps', 'Gbit/s', 64, '3.36'],
  ];
  it.each(CROSS)('across families (%#): %s %s -> %s at %d B shows %s', (units, text, from, to, frame, shown) => {
    expect(changeUnit(uv(text, at(units, from)), units, at(units, to), frame).text).toBe(shown);
  });

  it('across families without a frame size the number is cleared; an empty one stays empty', () => {
    expect(changeUnit(uv('100', at(INIT_RATE_UNITS, 'Gbit/s')), INIT_RATE_UNITS, at(INIT_RATE_UNITS, 'Mpps'))).toEqual(
      uv('', at(INIT_RATE_UNITS, 'Mpps')),
    );
    const empty = changeUnit(uv('', at(INIT_RATE_UNITS, 'Gbit/s')), INIT_RATE_UNITS, at(INIT_RATE_UNITS, 'Mpps'), 1518);
    expect(empty.text).toBe('');
  });

  it('a cross-family change remembers its source: going back shows the original, and saving uses it, not the rounded number', () => {
    const gbit = at(INIT_RATE_UNITS, 'Gbit/s');
    const mpps = at(INIT_RATE_UNITS, 'Mpps');
    const shown = changeUnit(uv('100', gbit), INIT_RATE_UNITS, mpps, 1518);
    expect(shown.text).toBe('8.127');
    expect(changeUnit(shown, INIT_RATE_UNITS, gbit, 1518).text).toBe('100');
    expect(
      cfgOf(full({ initialization_rate_pps: shown, avg_frame_size: '1518' })).delayer?.initialization_rate_pps,
    ).toBe(8127438);
  });

  it('parseFrame accepts 64 and more (decimals too) and nothing else', () => {
    expect([parseFrame('64'), parseFrame('1518.5')]).toEqual([64, 1518.5]);
    for (const bad of ['63', '0', '-100', '', 'abc']) expect(parseFrame(bad)).toBeNull();
  });
});

describe('formToDto: what is sent, in base units', () => {
  it('a new form with no stage is Buffer, Statistics and the default histograms: nothing else, no frame size', () => {
    expect(formToDto(defaultConfigForm())).toStrictEqual({
      config: {
        buffer_capacity: 100000,
        reordering: false,
        delayer: null,
        limiter: null,
        loss_model: null,
        seed: null,
        histogram: DELAY_DEFAULT,
        iat_histogram: IAT_DEFAULT,
        delay_stats: true,
      },
    });
  });

  it('the default values of the stages are sent untouched in base units', () => {
    const c = cfgOf(full());
    expect(c.delayer).toEqual({
      distribution: 'NormalDistribution',
      mean_ns: 1e6,
      std_ns: 1e5,
      activate_correlation: false,
      half_life_period_ns: 0,
      initialization_rate_pps: 1e7,
    });
    expect(c.limiter).toStrictEqual({ mode: 'Spacer', rate_bps: 1e10 });
    expect(c.loss_model).toStrictEqual({ type: 'Random', p: 0 });
  });

  const delayerOf = (c: ConfigDto) => must(c.delayer);
  // [what, form change, where the value ends up, expected base value]
  const PAYLOAD: [string, Partial<ConfigForm>, (c: ConfigDto) => number | null | undefined, number][] = [
    ['2.5 µs mean', { mean_ns: uv('2.5', 1) }, (c) => delayerOf(c).mean_ns, 2500],
    ['0.1 ms mean', { mean_ns: uv('0.1', 2) }, (c) => delayerOf(c).mean_ns, 100000],
    ['1.005 ms mean', { mean_ns: uv('1.005', 2) }, (c) => delayerOf(c).mean_ns, 1005000],
    ['1.001 µs mean', { mean_ns: uv('1.001', 1) }, (c) => delayerOf(c).mean_ns, 1001],
    ['0.001 s mean', { mean_ns: uv('0.001', 3) }, (c) => delayerOf(c).mean_ns, 1000000],
    ['1.5 ns mean (f64 on the wire, not rounded)', { mean_ns: uv('1.5', 0) }, (c) => delayerOf(c).mean_ns, 1.5],
    ['0.25 ms std', { std_ns: uv('0.25', 2) }, (c) => delayerOf(c).std_ns, 250000],
    [
      '1.1 µs half-life',
      { activate_correlation: true, half_life_period_ns: uv('1.1', 1) },
      (c) => delayerOf(c).half_life_period_ns,
      1100,
    ],
    ['1.005 ms histogram lo', { hist_lo: uv('1.005', 2) }, (c) => c.histogram?.lo_ns, 1005000],
    ['2 s histogram hi', { hist_hi: uv('2', 3) }, (c) => c.histogram?.hi_ns, 2e9],
    ['100 ns IAT lo', { iat_lo: uv('100', 0) }, (c) => c.iat_histogram?.lo_ns, 100],
    ['2 s IAT hi', { iat_hi: uv('2', 3) }, (c) => c.iat_histogram?.hi_ns, 2e9],
    ['1.1 kbit/s limiter rate', { rate_bps: uv('1.1', 1) }, (c) => c.limiter?.rate_bps, 1100],
    ['0.067 Gbit/s limiter rate', { rate_bps: uv('0.067', GBIT) }, (c) => c.limiter?.rate_bps, 67000000],
    ['0.267 Gbit/s limiter rate', { rate_bps: uv('0.267', GBIT) }, (c) => c.limiter?.rate_bps, 267000000],
    ['1.5 pps', { initialization_rate_pps: uv('1.5', 0) }, (c) => delayerOf(c).initialization_rate_pps, 1.5],
    ['1500 kpps', { initialization_rate_pps: uv('1500', 1) }, (c) => delayerOf(c).initialization_rate_pps, 1.5e6],
    ['12.5 Mpps', { initialization_rate_pps: uv('12.5', MPPS) }, (c) => delayerOf(c).initialization_rate_pps, 12.5e6],
  ];
  it.each(PAYLOAD)('%s is sent as the exact base value', (_what, over, pick, want) => {
    expect(pick(cfgOf(full(over)))).toBe(want);
  });

  it('a stage that is switched off is sent as null and its typed values are not validated', () => {
    const c = cfgOf(
      form({ mean_ns: uv('', 2), rate_bps: uv('-3', 0), random_p: 'abc', initialization_rate_pps: uv('', MPPS) }),
    );
    expect([c.delayer, c.limiter, c.loss_model]).toEqual([null, null, null]);
  });

  it('the seed is a whole number up to 2^53 - 1, empty is none', () => {
    expect(cfgOf(form({ seed: '' })).seed).toBeNull();
    expect(cfgOf(form({ seed: '42' })).seed).toBe(42);
    expect(cfgOf(form({ seed: '9007199254740991' })).seed).toBe(9007199254740991);
    for (const bad of ['1.5', '-1', 'abc', '9007199254740992', '1e3']) expectRefused(form({ seed: bad }), /Seed/);
  });
});

describe('formToDto: buffer capacity, required fields, negatives and probabilities', () => {
  it.each([
    ['1500', 'packets', 1500],
    ['1.5', 'kpackets', 1500],
    ['1.001', 'kpackets', 1001],
    ['0.001', 'Mpackets', 1000],
    ['2', 'Mpackets', 2000000],
    ['0', 'kpackets', 0],
  ])('buffer capacity %s %s is the integer packet count %d', (text, unit, packets) => {
    expect(cfgOf(form({ buffer_capacity: uv(text, at(PACKET_UNITS, unit)) })).buffer_capacity).toBe(packets);
  });

  it.each([
    ['1.0005', 'kpackets'],
    ['0.5', 'packets'],
    ['0.0000005', 'Mpackets'],
    ['-1', 'packets'],
    ['', 'packets'],
    ['abc', 'packets'],
  ])('buffer capacity %j %s is refused with a message naming the field', (text, unit) => {
    expectRefused(form({ buffer_capacity: uv(text, at(PACKET_UNITS, unit)) }), /Buffer capacity/);
  });

  it.each([
    ['mean delay', { mean_ns: uv('', MS) }, /Mean delay/],
    ['delay std', { std_ns: uv('', MS) }, /Delay std/],
    ['half-life while correlation is on', { activate_correlation: true, half_life_period_ns: uv('', MS) }, /Half-life/],
    ['limiter rate', { rate_bps: uv('', GBIT) }, /\bRate\b/],
    ['initialization rate', { initialization_rate_pps: uv('', MPPS) }, /Initialization rate/],
  ] as [string, Partial<ConfigForm>, RegExp][])('an empty %s is refused', (_what, over, field) => {
    expectRefused(full(over), field);
  });

  it('an empty std (deterministic distribution) and an empty half-life (no correlation) are fine and sent as 0', () => {
    const d = must(
      cfgOf(full({ distribution: 'DeterministicDistribution', std_ns: uv('', MS), half_life_period_ns: uv('', MS) }))
        .delayer,
    );
    expect([d.std_ns, d.half_life_period_ns]).toEqual([0, 0]);
  });

  it('the conditional rules: std is required unless deterministic, half-life only with correlation', () => {
    expect(stdRequired(form({ distribution: 'NormalDistribution' }))).toBe(true);
    expect(stdRequired(form({ distribution: 'DeterministicDistribution' }))).toBe(false);
    expect(halfLifeRequired(form({ activate_correlation: true }))).toBe(true);
    expect(halfLifeRequired(form())).toBe(false);
  });

  it.each([
    ['mean delay', { mean_ns: uv('-5', MS) }, /Mean delay/],
    ['delay std', { std_ns: uv('-1', 0) }, /Delay std/],
    [
      'half-life while correlation is on',
      { activate_correlation: true, half_life_period_ns: uv('-1', 0) },
      /Half-life/,
    ],
    ['limiter rate', { rate_bps: uv('-1', 0) }, /\bRate\b/],
    ['histogram lo', { hist_lo: uv('-1', 0) }, /Histogram lo/],
    ['IAT histogram hi', { iat_hi: uv('-1', 0) }, /IAT histogram hi/],
  ] as [string, Partial<ConfigForm>, RegExp][])('a negative %s is refused, naming the field', (_what, over, field) => {
    expect(refusal(full(over))).toMatch(field);
    expectRefused(full(over), /negative/);
  });

  it('zero is a valid mean delay, std and buffer capacity', () => {
    const c = cfgOf(full({ mean_ns: uv('0', MS), std_ns: uv('0', MS), buffer_capacity: uv('0', PACKETS) }));
    expect([must(c.delayer).mean_ns, must(c.delayer).std_ns, c.buffer_capacity]).toEqual([0, 0, 0]);
  });

  it.each(['-0.1', '-1', '1.5', '2', '1.0001', '', 'abc'])('a random loss probability of %j is refused', (p) => {
    expectRefused(full({ random_p: p }), /Random loss probability/);
  });
  it.each(['0', '1', '0.5', '0.000001', '0.999999'])('a random loss probability of %s is accepted', (p) => {
    expect(cfgOf(full({ random_p: p })).loss_model).toEqual({ type: 'Random', p: Number(p) });
  });

  const GE = [
    ['ge_p', /Gilbert-Elliot p/],
    ['ge_r', /Gilbert-Elliot r/],
    ['ge_k', /Gilbert-Elliot k/],
    ['ge_h', /Gilbert-Elliot h/],
  ] as const;
  it.each(GE.flatMap(([key, name]) => ['-0.1', '1.5', ''].map((v) => [key, name, v] as const)))(
    'Gilbert-Elliot %s = %j is refused',
    (key, name, v) => {
      expectRefused(full({ loss_type: 'GilbertElliot', [key]: v }), name);
    },
  );
  it('Gilbert-Elliot accepts 0 and 1 in every parameter; only the chosen model is checked', () => {
    const ge = { loss_type: 'GilbertElliot', ge_p: '0', ge_r: '1', ge_k: '1', ge_h: '0', random_p: '7' } as const;
    expect(cfgOf(full(ge)).loss_model).toEqual({ type: 'GilbertElliot', p: 0, r: 1, k: 1, h: 0 });
    expect(cfgOf(full({ ...ge, loss_type: 'Random', random_p: '0.5', ge_p: '9' })).loss_model).toEqual({
      type: 'Random',
      p: 0.5,
    });
  });
});

describe('formToDto: rates and the average frame size', () => {
  const gbit = at(INIT_RATE_UNITS, 'Gbit/s');
  const limMpps = at(LIMITER_RATE_UNITS, 'Mpps');

  it.each([
    ['0.5', 'Mpps', 500000],
    ['1', 'pps', 1],
    ['0.000001', 'Mpps', 1],
    ['0.001', 'kpps', 1],
    ['2.5', 'kpps', 2500],
  ])('initialization rate %s %s is accepted (at least 1 pps) and sent as %d', (text, unit, pps) => {
    expect(
      cfgOf(full({ initialization_rate_pps: uv(text, at(INIT_RATE_UNITS, unit)) })).delayer?.initialization_rate_pps,
    ).toBe(pps);
  });

  it.each([
    ['0.9', 'pps'],
    ['0.0000009', 'Mpps'],
    ['0.0009', 'kpps'],
    ['0', 'Mpps'],
    ['-2', 'kpps'],
  ])('initialization rate %s %s is below 1 pps and refused', (text, unit) => {
    expectRefused(full({ initialization_rate_pps: uv(text, at(INIT_RATE_UNITS, unit)) }), /Initialization rate/);
  });

  it.each([
    ['0', /greater than 0/],
    ['-1', /negative/],
    ['', /Rate/],
  ])('a limiter rate of %j is refused', (text, why) => {
    const m = refusal(full({ rate_bps: uv(text, 0) }));
    expect(m).toMatch(/\bRate\b/);
    expect(m).toMatch(why);
  });
  it('a limiter rate of 1 bit/s is the smallest valid one', () => {
    expect(cfgOf(full({ rate_bps: uv('1', 0) })).limiter?.rate_bps).toBe(1);
  });

  // [field, form change, frame, stored base value]: a value typed in the other family is converted and rounded to a whole unit
  const CONVERTED: [string, Partial<ConfigForm>, string, number][] = [
    ['100 Gbit/s init rate', { initialization_rate_pps: uv('100', gbit) }, '1518', 8127438],
    ['1.5 Gbit/s init rate (nearest, not down)', { initialization_rate_pps: uv('1.5', gbit) }, '1518', 121912],
    ['0.1 Gbit/s init rate', { initialization_rate_pps: uv('0.1', gbit) }, '1518', 8127],
  ];
  it.each(CONVERTED)('%s at %s B: stored as %d pps', (_what, over, frame, pps) => {
    const f = full({ ...over, avg_frame_size: frame });
    expect(cfgOf(f).delayer?.initialization_rate_pps).toBe(pps);
  });

  it('a limiter rate typed in pps is converted to a whole bit/s (8 Mpps at 1518 B; 1 pps at 100.1 B is 961)', () => {
    expect(cfgOf(full({ rate_bps: uv('8', limMpps), avg_frame_size: '1518' })).limiter?.rate_bps).toBe(98432000000);
    expect(
      cfgOf(full({ rate_bps: uv('1', at(LIMITER_RATE_UNITS, 'pps')), avg_frame_size: '100.1' })).limiter?.rate_bps,
    ).toBe(961);
  });

  it('the >= 1 pps and > 0 bit/s minimums apply after conversion', () => {
    expectRefused(
      full({ initialization_rate_pps: uv('5000', at(INIT_RATE_UNITS, 'bit/s')), avg_frame_size: '1518' }),
      /Initialization rate/,
    );
    const pps = at(LIMITER_RATE_UNITS, 'pps');
    expectRefused(full({ rate_bps: uv('0.00001', pps), avg_frame_size: '1518' }), /\bRate\b/); // 0.12 bit/s
    expect(cfgOf(full({ rate_bps: uv('0.0001', pps), avg_frame_size: '1518' })).limiter?.rate_bps).toBe(1); // 1.23 bit/s
  });

  it('a value in the own family is stored unchanged whatever the frame size says', () => {
    expect(cfgOf(full({ avg_frame_size: '64' })).limiter?.rate_bps).toBe(1e10);
  });

  it('the frame size is stored beside the config in bytes, left out when empty, and refused when invalid even if unused', () => {
    expect(formToDto(full({ avg_frame_size: '1518' }))).toMatchObject({ avg_frame_size_byte: 1518 });
    expect(cfgOf(full({ avg_frame_size: '1518' }))).not.toHaveProperty('avg_frame_size_byte');
    expect(formToDto(full())).not.toHaveProperty('avg_frame_size_byte');
    for (const bad of ['63', '0', '-100', 'abc']) expectRefused(full({ avg_frame_size: bad }), /Average frame size/);
  });

  it('a rate field in the other family without a (valid) frame size is refused, naming the frame size', () => {
    const cross = { initialization_rate_pps: uv('100', gbit) };
    expectRefused(full(cross), /Average frame size/);
    expectRefused(full({ ...cross, avg_frame_size: '63' }), /Average frame size/);
    expectRefused(full({ rate_bps: uv('8', limMpps) }), /Average frame size/);
  });

  it('the frame size is required exactly while an added stage shows a rate unit of the other family', () => {
    expect(frameRequired(full())).toBe(false);
    expect(frameRequired(full({ initialization_rate_pps: uv('1', gbit) }))).toBe(true);
    expect(frameRequired(full({ rate_bps: uv('1', limMpps) }))).toBe(true);
    expect(frameRequired(form({ initialization_rate_pps: uv('1', gbit), rate_bps: uv('1', limMpps) }))).toBe(false); // stages off
  });
});

describe('formToDto: delay accuracy', () => {
  it('Basic, Reordering and Correlation are one choice over the two booleans; Reordering wins', () => {
    const base = form();
    expect(accuracyOf(base)).toBe('Basic');
    expect(accuracyOf(withAccuracy(base, 'Correlation'))).toBe('Correlation');
    expect(accuracyOf(withAccuracy(base, 'Reordering'))).toBe('Reordering');
    expect(accuracyOf(form({ reordering: true, activate_correlation: true }))).toBe('Reordering');
    const back = withAccuracy(withAccuracy(base, 'Reordering'), 'Basic');
    expect([back.reordering, back.activate_correlation]).toEqual([false, false]);
  });

  it('picking Reordering replaces the Spacer by the token bucket and keeps the rate; the other options leave the mode alone', () => {
    const r = withAccuracy(form({ limiter_mode: 'Spacer', rate_bps: uv('5', GBIT) }), 'Reordering');
    expect(r.limiter_mode).toBe('TokenBucketFilter');
    expect(r.rate_bps).toEqual(uv('5', GBIT));
    expect(withAccuracy(form({ limiter_mode: 'Spacer' }), 'Correlation').limiter_mode).toBe('Spacer');
    expect(withAccuracy(form({ limiter_mode: 'TokenBucketFilter' }), 'Basic').limiter_mode).toBe('TokenBucketFilter');
  });

  it('Reordering with the Spacer limiter is refused; with the token bucket, or without a limiter, it is sent', () => {
    expect(refusal(full({ reordering: true, limiter_mode: 'Spacer' }))).toMatch(/Reordering.*Spacer/);
    expect(cfgOf(full({ reordering: true, limiter_mode: 'TokenBucketFilter' })).reordering).toBe(true);
    const noLimiter = cfgOf(full({ reordering: true, limiter_added: false }));
    expect([noLimiter.reordering, noLimiter.limiter]).toEqual([true, null]);
  });

  it('Reordering together with correlation is refused', () => {
    expect(refusal(full({ reordering: true, activate_correlation: true, limiter_mode: 'TokenBucketFilter' }))).toMatch(
      /Reordering.*correlation/,
    );
  });

  it('without the Delayer stage the config is sent with Basic, whatever was chosen', () => {
    const f = form({ reordering: true, limiter_mode: 'Spacer', limiter_added: true });
    expect(reorderingActive(f)).toBe(false);
    expect(cfgOf(f).reordering).toBe(false);
  });

  it('Correlation keeps its half-life requirement, Reordering has none', () => {
    expectRefused(full({ activate_correlation: true, half_life_period_ns: uv('', MS) }), /Half-life/);
    expect(cfgOf(full({ activate_correlation: true, limiter_mode: 'Spacer' })).delayer?.activate_correlation).toBe(
      true,
    );
    expect(
      cfgOf(full({ reordering: true, limiter_mode: 'TokenBucketFilter', half_life_period_ns: uv('', MS) })).reordering,
    ).toBe(true);
  });
});

describe('formToDto: histograms', () => {
  it('a new form has the six P4TG defaults, shown in the largest fitting unit (0 in the default unit)', () => {
    const f = defaultConfigForm();
    const us = at(TIME_UNITS, MU);
    expect([f.hist_bins, f.hist_lo, f.hist_hi]).toEqual(['16', uv('1.024', us), uv('2.048', us)]);
    expect([f.iat_bins, f.iat_lo, f.iat_hi]).toEqual(['16', uv('0', MS), uv('1.024', us)]);
  });

  it('empty fields are sent as the explicit defaults, never null; delay statistics pass through either way', () => {
    const c = cfgOf(form({ ...emptyHistogramFields, delay_stats: false }));
    expect(c.histogram).toStrictEqual(DELAY_DEFAULT);
    expect(c.iat_histogram).toStrictEqual(IAT_DEFAULT);
    expect(c.delay_stats).toBe(false);
  });

  it('bins, lo and hi are sent as typed in base units; each empty one is its own default', () => {
    expect(cfgOf(form({ hist_bins: '64', hist_lo: uv('1', 2), hist_hi: uv('9', 1) })).histogram).toEqual({
      bins: 64,
      lo_ns: 1e6,
      hi_ns: 9000,
    });
    const only = (over: Partial<ConfigForm>) => cfgOf(form({ ...emptyHistogramFields, ...over }));
    expect(only({ iat_bins: '4096' }).iat_histogram).toEqual({ ...IAT_DEFAULT, bins: 4096 });
    expect(only({ hist_bins: '4096' }).histogram).toEqual({ ...DELAY_DEFAULT, bins: 4096 }); // the cap is the controller's
    expect(only({ hist_hi: uv('3', 2) }).histogram).toEqual({ ...DELAY_DEFAULT, hi_ns: 3e6 });
  });

  it('an explicit lo of 0 is a value, not "empty"', () => {
    expect(cfgOf(form({ hist_lo: uv('0', 0) })).histogram).toEqual({ ...DELAY_DEFAULT, lo_ns: 0 });
  });

  it('the two histograms do not leak into each other', () => {
    const c = cfgOf(form({ hist_bins: '64', iat_bins: '128', iat_hi: uv('2', 3) }));
    expect(c.histogram).toEqual({ ...DELAY_DEFAULT, bins: 64 });
    expect(c.iat_histogram).toEqual({ ...IAT_DEFAULT, bins: 128, hi_ns: 2e9 });
  });

  it('a bad value names its own histogram: IAT fields say "IAT histogram", delay fields say "Histogram"', () => {
    expectRefused(form({ iat_bins: '1.5' }), /IAT histogram bins/);
    expectRefused(form({ iat_bins: 'abc' }), /IAT histogram bins/);
    expect(refusal(form({ hist_lo: uv('-1', 0) }))).toMatch(/^Histogram lo/);
    expect(refusal(form({ iat_lo: uv('-1', 0) }))).toMatch(/^IAT histogram lo/);
    expectRefused(form({ hist_bins: '1.5' }), /Histogram bins/);
  });
});

describe('configToForm and formToDto round-trip a stored config exactly', () => {
  const strip = (c: ConfigDto) => JSON.parse(JSON.stringify(dropNulls(c))) as unknown;

  it.each([
    ['config', fx.config({ histogram: DELAY_DEFAULT })],
    ['config2 (every stage, Gilbert-Elliott, histogram)', fx.config2()],
    [
      'config3 with the token bucket (its Spacer is display-only)',
      { ...fx.config3(), limiter: { mode: 'TokenBucketFilter' as const, rate_bps: 5e9 } },
    ],
    ['no stages', fx.configBare({ histogram: DELAY_DEFAULT })],
  ])('%s', (_n, c) => {
    expect(strip(cfgOf(configToForm(c)))).toEqual(strip({ iat_histogram: IAT_DEFAULT, ...c }));
  });

  it('a stored histogram or IAT histogram that is null, absent or has a null member opens with the defaults there', () => {
    const { histogram: _h, ...absent } = fx.config();
    const nullMembers = { bins: null, lo_ns: null, hi_ns: null };
    for (const c of [
      fx.config({ histogram: null, iat_histogram: null }),
      absent as ConfigDto,
      fx.config({ histogram: nullMembers, iat_histogram: {} }),
    ]) {
      const f = configToForm(c);
      expect(f.hist_bins).toBe('16');
      expect(cfgOf(f).histogram).toStrictEqual(DELAY_DEFAULT);
      expect(cfgOf(f).iat_histogram).toStrictEqual(IAT_DEFAULT);
    }
    const partial = cfgOf(configToForm(fx.config({ histogram: { bins: 64, lo_ns: null, hi_ns: 5000 } })));
    expect(partial.histogram).toStrictEqual({ bins: 64, lo_ns: 1024, hi_ns: 5000 });
  });

  const withDelayer = (over: Partial<NonNullable<ConfigDto['delayer']>>): ConfigDto => ({
    ...fx.config(),
    delayer: { ...must(fx.config().delayer), ...over },
  });
  it('time values: every display unit and back', () => {
    for (const ns of [0, 1, 999, 1000, 2500, 1001, 12345, 1234567, 1000500, 999999999, 1005000, 8200000, 2000000000]) {
      const c = withDelayer({ mean_ns: ns, std_ns: ns, half_life_period_ns: ns });
      expect(cfgOf(configToForm(c)).delayer).toEqual(c.delayer);
    }
  });

  it('rates: limiter bit/s and initialization pps keep their exact value', () => {
    for (const bps of [999, 1500, 2500000, 67000000, 1234567000, 40000000000]) {
      const c = { ...fx.config(), limiter: { mode: 'Spacer' as const, rate_bps: bps } };
      expect(cfgOf(configToForm(c)).limiter?.rate_bps).toBe(bps);
    }
    for (const pps of [1, 1.5, 999, 1500, 6543210, 12500000]) {
      expect(cfgOf(configToForm(withDelayer({ initialization_rate_pps: pps }))).delayer?.initialization_rate_pps).toBe(
        pps,
      );
    }
  });

  it('histogram and IAT histogram bounds in their own units, and buffer capacities', () => {
    const c: ConfigDto = {
      ...fx.config(),
      histogram: { bins: 64, lo_ns: 1001, hi_ns: 2500000 },
      iat_histogram: { bins: 32, lo_ns: 100, hi_ns: 9000 },
    };
    expect(strip(cfgOf(configToForm(c)))).toEqual(strip(c));
    for (const n of [0, 1, 999, 1500, 100000, 2000000]) {
      expect(cfgOf(configToForm({ ...fx.config(), buffer_capacity: n })).buffer_capacity).toBe(n);
    }
  });

  it('a stored reordering without a delayer is shown but sent back with Basic', () => {
    const c = { ...fx.configBare(), reordering: true };
    expect(configToForm(c).reordering).toBe(true);
    expect(cfgOf(configToForm(c)).reordering).toBe(false);
  });

  it('the stored frame size comes back beside the config; none (null or absent) stays none', () => {
    expect(formToDto(configToForm(fx.config(), 1518)).avg_frame_size_byte).toBe(1518);
    for (const none of [null, undefined])
      expect(formToDto(configToForm(fx.config(), none))).not.toHaveProperty('avg_frame_size_byte');
  });

  it('a stored stage that is null or absent is not added', () => {
    const { limiter: _l, ...absent } = fx.config();
    for (const c of [fx.config(), absent as ConfigDto]) {
      const f = configToForm(c);
      expect([f.delayer_added, f.limiter_added, f.dropper_added]).toEqual([true, false, true]);
    }
  });
});

describe('link rules: ports and what the form accepts unchanged', () => {
  it.each(['0000:65:00.0', '0000:b3:00.7', 'net_null0', 'net_ring12'])('the port %s is valid', (p) => {
    expect(portError(p)).toBeNull();
  });
  it.each(['', 'eth0', '0000:65:00.8', '65:00.0', 'net_null', 'net_tap0'])('the port %j is refused, naming it', (p) => {
    expect(portError(p)).toContain(`"${p}"`);
  });

  it('the fixtures are valid links', () => {
    for (const l of [fx.link(), fx.linkRev(), fx.loopLink()]) expect(linkProblem(l)).toBeNull();
  });

  it('an extra name member is no problem, whatever it holds (links have no names any more)', () => {
    for (const name of ['old', '', 'has space', 'x'.repeat(40)]) expect(linkProblem({ ...fx.link(), name })).toBeNull();
  });

  it.each([
    ['not an object', 'x', /not an object/],
    ['id 0', { ...fx.link(), id: 0 }, /id/i],
    ['id 1.5', { ...fx.link(), id: 1.5 }, /id/i],
    ['a string id', { ...fx.link(), id: '1' }, /id/i],
    ['no ingress', { ...fx.link(), ingress: '' }, /Ingress/],
    ['no egress', { ...fx.link(), egress: undefined }, /Egress/],
    ['a frame size below 64', { ...fx.link(), avg_frame_size_byte: 63 }, /frame/i],
    ['a string frame size', { ...fx.link(), avg_frame_size_byte: '1518' }, /frame/i],
    ['no config', { ...fx.link(), config: undefined }, /config/i],
    [
      'a limiter rate of 0',
      { ...fx.link(), config: { ...fx.config(), limiter: { mode: 'Spacer', rate_bps: 0 } } },
      /Rate/,
    ],
  ])('%s is a problem', (_what, l, why) => {
    expect(linkProblem(l)).toMatch(why);
  });

  it('a stored link that breaks only the delay accuracy rule stays valid, so it can be opened and fixed', () => {
    const broken = {
      ...fx.link(),
      config: { ...fx.config(), reordering: true, limiter: { mode: 'Spacer' as const, rate_bps: 1e9 } },
    };
    expect(linkProblem(broken)).toBeNull();
    expect(() => formToDto(configToForm(broken.config))).toThrow(/Reordering/);
  });
});

describe('the derived limit', () => {
  it('the derived limit is capacity x 1542 B x 8 bit; no number while the capacity is no valid count', () => {
    expect(derivedLimitText('100000')).toContain('1233.6 Mbit');
    expect(derivedLimitText('1000')).toContain('12.336 Mbit');
    for (const bad of ['', 'abc', '-1', '1.5']) {
      expect(derivedLimitText(bad)).not.toMatch(/=|NaN|\d+ [kMG]?bit/);
    }
  });
});

describe('rangeAdvice: advisory hints on the histogram range', () => {
  const NS = at(TIME_UNITS, 'ns');
  const MS = at(TIME_UNITS, 'ms');
  const MUS = at(TIME_UNITS, MU);
  const advice = (bins: string, lo: UnitValue, hi: UnitValue) =>
    rangeAdvice(form({ hist_bins: bins, hist_lo: lo, hist_hi: hi }));

  it('a power-of-two bin width that divides lo is aligned, also below 1 ns', () => {
    expect(advice('64', uv('0', NS), uv('4.096', MUS))).toMatchObject({ aligned: true });
    expect(advice('4096', uv('0', NS), uv('1024', NS))).toMatchObject({ aligned: true });
  });

  it('otherwise the next power of two for hi is suggested, in the unit of hi', () => {
    expect(advice('', uv('', MS), uv('3', MUS))).toMatchObject({ aligned: false, suggested: '4.096' });
    expect(advice('100', uv('1', MS), uv('7', MS))).toMatchObject({ aligned: false });
  });

  it('there is no advice without hi, or when hi is already a power of two over lo = 0', () => {
    expect(advice('64', uv('0', NS), uv('', MS))).toBeNull();
    expect(advice('', uv('0', NS), uv('4096', NS))).toBeNull();
  });
});
