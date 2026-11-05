import { screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import * as fx from '../test/fixtures';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  PPS_UNITS,
  RATE_UNITS,
  SIZE_UNITS,
  TIME_UNITS,
  numberField,
  pickUnit,
  renderApp,
  section,
  seedLinks,
  shownBase,
  typeInto,
  typeRate,
  unitOf,
  unitSelect,
} from '../test/helpers';
import { must } from '../test/must';

// Contract: "v16" block of src/test/helpers.tsx. Every test runs against the link editor (the only page with the config form
// since spec27). Stored/base values are ns, bit/s, bit and pps; the form shows units.
const MU = 'µs'; // micro sign U+00B5, exactly as in the spec / P4TG

const UNIT_FIELDS: [string, string[]][] = [
  ['Mean delay', TIME_UNITS],
  ['Delay std', TIME_UNITS],
  ['Half-life period', TIME_UNITS],
  ['Histogram lo', TIME_UNITS],
  ['Histogram hi', TIME_UNITS],
  ['Rate', RATE_UNITS],
  ['Initialization rate', PPS_UNITS],
];
const SECTION_OF: Record<string, string> = {
  'Mean delay': 'Delayer',
  'Delay std': 'Delayer',
  'Half-life period': 'Delayer',
  'Initialization rate': 'Delayer',
  Rate: 'Limiter',
  'Histogram lo': 'Statistics',
  'Histogram hi': 'Statistics',
};
const body = () => document.body;

describe('v16 constants of the contract', () => {
  it('pins the exact unit labels (micro sign U+00B5, never "us")', () => {
    expect(TIME_UNITS).toEqual(['ns', MU, 'ms', 's']);
    expect(TIME_UNITS[1].charCodeAt(0)).toBe(0xb5);
    expect(RATE_UNITS).toEqual(['bit/s', 'kbit/s', 'Mbit/s', 'Gbit/s']);
    expect(PPS_UNITS).toEqual(['pps', 'kpps', 'Mpps']);
    expect(SIZE_UNITS).toEqual(['bit', 'kbit', 'Mbit', 'Gbit']);
  });
});

describe.each(PAGES)('$name: unit selects', (page: ConfigPage) => {
  it.each(UNIT_FIELDS)(
    '%s has exactly one unit select named "<label> unit" with the contracted options',
    async (label, units) => {
      await page.openFull();
      const sel = unitSelect(body(), label);
      expect(sel.tagName).toBe('SELECT');
      expect(screen.getAllByLabelText(`${label} unit`)).toHaveLength(1);
      expect(
        within(sel)
          .getAllByRole('option')
          .map((o) => o.textContent),
      ).toEqual(units);
      expect(section(SECTION_OF[label])).toContainElement(sel);
    },
  );

  it.each(UNIT_FIELDS)('the unit select of %s is the next form control right of its input', async (label) => {
    await page.openFull();
    const controls = Array.from(document.querySelectorAll('input, select, textarea, button'));
    const i = controls.indexOf(numberField(body(), label));
    expect(i).toBeGreaterThanOrEqual(0);
    expect(controls[i + 1]).toBe(unitSelect(body(), label));
  });

  it('only those seven fields have a unit select (no unit for capacity, seed, bins or probabilities)', async () => {
    await page.openFull();
    expect(screen.getAllByLabelText(/ unit$/)).toHaveLength(UNIT_FIELDS.length);
    for (const none of ['Buffer capacity', 'Seed', 'Histogram bins', 'Random loss probability']) {
      expect(screen.queryByLabelText(`${none} unit`)).not.toBeInTheDocument();
    }
  });

  it('no label carries a unit suffix any more', async () => {
    await page.openFull();
    for (const old of [
      'Mean delay (ns)',
      'Delay std (ns)',
      'Half-life period (ns)',
      'Initialization rate (pps)',
      'Rate (bit/s)',
      'Histogram lo (ns)',
      'Histogram hi (ns)',
    ]) {
      expect(screen.queryByLabelText(old)).not.toBeInTheDocument();
    }
  });
});

describe.each(PAGES)('$name: defaults of a new form', (page: ConfigPage) => {
  it('the initialization rate starts EMPTY with the unit select on Mpps', async () => {
    await page.openFull();
    expect(numberField(body(), 'Initialization rate')).toHaveValue(null);
    expect(unitOf(body(), 'Initialization rate')).toBe('Mpps');
  });

  it('empty or zero fields show the default units: half-life ms, histogram lo/hi ms', async () => {
    await page.openFull();
    expect(unitOf(body(), 'Half-life period')).toBe('ms');
    expect(numberField(body(), 'Half-life period')).toHaveValue(0);
    for (const l of ['Histogram lo', 'Histogram hi']) {
      expect(numberField(body(), l)).toHaveValue(null);
      expect(unitOf(body(), l)).toBe('ms');
    }
  });

  it('keeps the old default values: mean 1 ms, std 1e5 ns, rate 10 Gbit/s, capacity 100000', async () => {
    await page.openFull();
    expect(numberField(body(), 'Mean delay')).toHaveValue(1);
    expect(unitOf(body(), 'Mean delay')).toBe('ms');
    expect(shownBase(body(), 'Delay std')).toBe(100000); // 100 us or 0.1 ms: the display is free
    expect(numberField(body(), 'Rate')).toHaveValue(10);
    expect(unitOf(body(), 'Rate')).toBe('Gbit/s');
    expect(numberField(body(), 'Buffer capacity')).toHaveValue(100000);
  });

  it('sends those defaults in base units untouched', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await page.save(user);
    const c = page.created();
    expect(must(c.delayer).mean_ns).toBe(1000000);
    expect(must(c.delayer).std_ns).toBe(100000);
    expect(must(c.delayer).half_life_period_ns).toBe(0);
    expect(must(c.delayer).initialization_rate_pps).toBe(10000000); // the helper's rate (TEST_RATE pps)
    expect(must(c.limiter).rate_bps).toBe(1e10);
    expect(c.buffer_capacity).toBe(100000);
    expect(c.histogram ?? null).toBeNull();
  });

  it('saving the untouched form is still refused for the empty initialization rate (unit default is not a value)', async () => {
    const user = await page.openFull();
    await user.type(screen.getByLabelText('Name'), 'x');
    if (page.name === 'link editor') {
      await user.selectOptions(screen.getByLabelText('Ingress'), '0000:65:00.0');
      await user.selectOptions(screen.getByLabelText('Egress'), '0000:65:00.1');
    }
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Initialization rate/);
    expect(page.requests()).toBe(0);
  });
});

// [stored base value, shown value, shown unit]
const TIME_CASES: [number, number, string][] = [
  [0, 0, 'ms'],
  [1, 1, 'ns'],
  [999, 999, 'ns'],
  [1000, 1, MU],
  [2500, 2.5, MU],
  [1001, 1.001, MU],
  [12345, 12.345, MU],
  [1234567, 1234.567, MU],
  [1000001, 1000.001, MU],
  [1000500, 1000.5, MU],
  [999999999, 999999.999, MU],
  [1000000, 1, 'ms'],
  [1500000, 1.5, 'ms'],
  [1005000, 1.005, 'ms'],
  [10000000, 10, 'ms'],
  [8200000, 8.2, 'ms'],
  [1000000000, 1, 's'],
  [1500000000, 1.5, 's'],
  [2000000000, 2, 's'],
];
// (no 0 bit/s row: the limiter Rate must be greater than 0 (spec25), so a stored 0 is an invalid link, spec27b item 1)
const RATE_CASES: [number, number, string][] = [
  [999, 999, 'bit/s'],
  [1500, 1.5, 'kbit/s'],
  [2500000, 2.5, 'Mbit/s'],
  [67000000, 67, 'Mbit/s'],
  [1234567000, 1234.567, 'Mbit/s'],
  [1000000000, 1, 'Gbit/s'],
  [40000000000, 40, 'Gbit/s'],
];
const PPS_CASES: [number, number, string][] = [
  [1, 1, 'pps'],
  [1.5, 1.5, 'pps'],
  [999, 999, 'pps'],
  [1500, 1.5, 'kpps'],
  [6543210, 6543.21, 'kpps'],
  [12500000, 12.5, 'Mpps'],
  [8000000, 8, 'Mpps'],
];

describe.each(PAGES)('$name: display unit of a stored value, and the exact round trip on save', (page: ConfigPage) => {
  async function roundTrip(config: ConfigDto, label: string, shown: number, unit: string) {
    const user = await page.openStored(config);
    expect(numberField(body(), label)).toHaveValue(shown);
    expect(unitOf(body(), label)).toBe(unit);
    await page.save(user);
    return page.updated();
  }

  it.each(TIME_CASES)('mean delay %d ns is shown as %d %s and saved back unchanged', async (ns, shown, unit) => {
    const c = await roundTrip(cfg({ mean_ns: ns }), 'Mean delay', shown, unit);
    expect(must(c.delayer).mean_ns).toBe(ns);
  });

  it.each([
    [1001, 1.001, MU],
    [1005000, 1.005, 'ms'],
    [0, 0, 'ms'],
    [10000000, 10, 'ms'],
  ] as [number, number, string][])(
    'delay std %d ns is shown as %d %s and saved back unchanged',
    async (ns, shown, unit) => {
      const c = await roundTrip(cfg({ std_ns: ns }), 'Delay std', shown, unit);
      expect(must(c.delayer).std_ns).toBe(ns);
    },
  );

  it.each([
    [4000000, 4, 'ms'],
    [555, 555, 'ns'],
    [0, 0, 'ms'],
    [2500, 2.5, MU],
  ] as [number, number, string][])(
    'half-life %d ns is shown as %d %s and saved back unchanged',
    async (ns, shown, unit) => {
      const c = await roundTrip(cfg({ half_life_period_ns: ns }), 'Half-life period', shown, unit);
      expect(must(c.delayer).half_life_period_ns).toBe(ns);
    },
  );

  it.each(RATE_CASES)('limiter rate %d bit/s is shown as %d %s and saved back unchanged', async (bps, shown, unit) => {
    const c = await roundTrip(cfg({}, { rate_bps: bps }), 'Rate', shown, unit);
    expect(must(c.limiter).rate_bps).toBe(bps);
  });

  it('a stored limiter rate of 0 bit/s is an invalid link: the document is corrupt, the editor shows the alert (spec27b)', async () => {
    seedLinks([fx.link({ config: cfg({}, { rate_bps: 0 }) })]);
    renderApp('/settings/1');
    expect(await screen.findByRole('alert')).toHaveTextContent(/local configuration/i);
    expect(screen.queryByLabelText('Rate')).not.toBeInTheDocument();
  });

  it.each(PPS_CASES)(
    'initialization rate %d pps is shown as %d %s and saved back unchanged',
    async (pps, shown, unit) => {
      const c = await roundTrip(cfg({ initialization_rate_pps: pps }), 'Initialization rate', shown, unit);
      expect(must(c.delayer).initialization_rate_pps).toBe(pps);
    },
  );

  it.each([
    [100, 100, 'ns', 9000, 9, MU],
    [1000000, 1, 'ms', 2500000, 2.5, 'ms'],
    [1001, 1.001, MU, 1005000, 1.005, 'ms'],
  ] as [number, number, string, number, number, string][])(
    'histogram lo %d ns / hi %d ns are shown in their own units and saved back unchanged',
    async (lo, shownLo, unitLo, hi, shownHi, unitHi) => {
      const user = await page.openStored(cfg({}, {}, { bins: 64, lo_ns: lo, hi_ns: hi }));
      expect(numberField(body(), 'Histogram lo')).toHaveValue(shownLo);
      expect(unitOf(body(), 'Histogram lo')).toBe(unitLo);
      expect(numberField(body(), 'Histogram hi')).toHaveValue(shownHi);
      expect(unitOf(body(), 'Histogram hi')).toBe(unitHi);
      await page.save(user);
      const h = page.updated().histogram;
      expect(h?.lo_ns).toBe(lo);
      expect(h?.hi_ns).toBe(hi);
      expect(h?.bins).toBe(64);
    },
  );

  it('two fields of one form pick their units independently (mean in ms, std in ns)', async () => {
    await page.openStored(cfg({ mean_ns: 10000000, std_ns: 300 }));
    expect(unitOf(body(), 'Mean delay')).toBe('ms');
    expect(unitOf(body(), 'Delay std')).toBe('ns');
  });
});

describe.each(PAGES)('$name: changing a unit converts the shown value', (page: ConfigPage) => {
  it('2500 ns -> 2.5 us -> 0.0025 ms -> 0.0000025 s -> back to 2500 ns, and the saved value never changes', async () => {
    const user = await page.openFull();
    await typeInto(user, body(), 'Mean delay', '2500', 'ns');
    await pickUnit(user, body(), 'Mean delay', MU);
    expect(numberField(body(), 'Mean delay')).toHaveValue(2.5);
    await pickUnit(user, body(), 'Mean delay', 'ms');
    expect(numberField(body(), 'Mean delay')).toHaveValue(0.0025);
    await pickUnit(user, body(), 'Mean delay', 's');
    expect(numberField(body(), 'Mean delay')).toHaveValue(0.0000025);
    await pickUnit(user, body(), 'Mean delay', 'ns');
    expect(numberField(body(), 'Mean delay')).toHaveValue(2500);
    await pickUnit(user, body(), 'Mean delay', MU);
    await page.ready(user);
    await page.save(user);
    expect(must(page.created().delayer).mean_ns).toBe(2500);
  });

  it.each([
    ['Mean delay', 8.2, 'ms', MU, 8200, 8200000], // 8.2 * 1e6 / 1e3 is 8199.999999999998 in floating point
    ['Mean delay', 1.005, 'ms', MU, 1005, 1005000],
    ['Mean delay', 0.1, 'ms', 'ns', 100000, 100000],
    ['Mean delay', 1.001, MU, 'ns', 1001, 1001],
    ['Delay std', 1.1, MU, 'ms', 0.0011, 1100],
    ['Delay std', 120, MU, 'ms', 0.12, 120000],
  ] as [string, number, string, string, number, number][])(
    '%s: %d %s converted to %s shows %d and still means %d ns',
    async (label, typed, from, to, shown, ns) => {
      const user = await page.openFull();
      await typeInto(user, body(), label, String(typed), from);
      await pickUnit(user, body(), label, to);
      expect(numberField(body(), label)).toHaveValue(shown);
      await page.ready(user);
      await page.save(user);
      const d = must(page.created().delayer);
      expect(label === 'Mean delay' ? d.mean_ns : d.std_ns).toBe(ns);
    },
  );

  it.each([
    ['Rate', 1.5, 'Gbit/s', 'Mbit/s', 1500],
    ['Rate', 0.067, 'Gbit/s', 'Mbit/s', 67],
    ['Rate', 1500, 'Mbit/s', 'Gbit/s', 1.5],
    ['Initialization rate', 12.5, 'Mpps', 'kpps', 12500],
    ['Initialization rate', 1.5, 'kpps', 'pps', 1500],
    ['Initialization rate', 1500, 'pps', 'Mpps', 0.0015],
  ] as [string, number, string, string, number][])(
    '%s: %d %s converted to %s shows %d',
    async (label, typed, from, to, shown) => {
      const user = await page.openFull();
      await typeInto(user, body(), label, String(typed), from);
      await pickUnit(user, body(), label, to);
      expect(numberField(body(), label)).toHaveValue(shown);
      expect(unitOf(body(), label)).toBe(to);
    },
  );

  it('half-life and histogram fields convert too', async () => {
    const user = await page.openFull();
    await user.click(screen.getByLabelText('Activate correlation'));
    await typeInto(user, body(), 'Half-life period', '4', 'ms');
    await pickUnit(user, body(), 'Half-life period', MU);
    expect(numberField(body(), 'Half-life period')).toHaveValue(4000);
    await typeInto(user, body(), 'Histogram lo', '2', 'ms');
    await pickUnit(user, body(), 'Histogram lo', 'ns');
    expect(numberField(body(), 'Histogram lo')).toHaveValue(2000000);
    await typeInto(user, body(), 'Histogram hi', '500', MU);
    await pickUnit(user, body(), 'Histogram hi', 'ms');
    expect(numberField(body(), 'Histogram hi')).toHaveValue(0.5);
  });

  it('an empty field stays empty when its unit changes (histogram lo, initialization rate)', async () => {
    const user = await page.openFull();
    await pickUnit(user, body(), 'Histogram lo', MU);
    expect(numberField(body(), 'Histogram lo')).toHaveValue(null);
    await pickUnit(user, body(), 'Initialization rate', 'kpps');
    expect(numberField(body(), 'Initialization rate')).toHaveValue(null);
    expect(unitOf(body(), 'Initialization rate')).toBe('kpps');
  });

  it('changing the unit of one field leaves the others alone', async () => {
    const user = await page.openFull();
    const before = shownBase(body(), 'Delay std');
    await pickUnit(user, body(), 'Mean delay', MU);
    expect(shownBase(body(), 'Delay std')).toBe(before);
    expect(unitOf(body(), 'Mean delay')).toBe(MU);
  });

  it('the converted value of a stored field is saved as the same base value (unit change does not edit it)', async () => {
    const user = await page.openStored(cfg({ mean_ns: 1234567 }));
    await pickUnit(user, body(), 'Mean delay', 'ns');
    expect(numberField(body(), 'Mean delay')).toHaveValue(1234567);
    await pickUnit(user, body(), 'Mean delay', 's');
    expect(numberField(body(), 'Mean delay')).toHaveValue(0.001234567);
    await page.save(user);
    expect(must(page.updated().delayer).mean_ns).toBe(1234567);
  });
});

type Pick = (c: ConfigDto) => number | null | undefined;
type Prep = 'correlation' | undefined;
// [label, unit, typed text, expected base value, where it ends up, switch that enables the field (v22: the limiter has none)]
const PAYLOAD: [string, string, string, number, Pick, Prep][] = [
  ['Mean delay', 'ns', '2500', 2500, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', MU, '2.5', 2500, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 'ms', '0.1', 100000, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 'ms', '1.005', 1005000, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 'ms', '8.2', 8200000, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', MU, '1.001', 1001, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 's', '1.5', 1500000000, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 's', '0.001', 1000000, (c) => must(c.delayer).mean_ns, undefined],
  ['Mean delay', 'ns', '1.5', 1.5, (c) => must(c.delayer).mean_ns, undefined], // f64 on the wire: no rounding to an integer
  ['Delay std', MU, '1.1', 1100, (c) => must(c.delayer).std_ns, undefined],
  ['Delay std', 'ms', '0.25', 250000, (c) => must(c.delayer).std_ns, undefined],
  ['Half-life period', 'ms', '2', 2000000, (c) => must(c.delayer).half_life_period_ns, 'correlation'],
  ['Half-life period', MU, '1.1', 1100, (c) => must(c.delayer).half_life_period_ns, 'correlation'],
  ['Histogram lo', 'ms', '1.005', 1005000, (c) => c.histogram?.lo_ns, undefined],
  ['Histogram lo', 'ns', '100', 100, (c) => c.histogram?.lo_ns, undefined],
  ['Histogram hi', 's', '2', 2000000000, (c) => c.histogram?.hi_ns, undefined],
  ['Histogram hi', MU, '9', 9000, (c) => c.histogram?.hi_ns, undefined],
  ['Rate', 'bit/s', '1000', 1000, (c) => must(c.limiter).rate_bps, undefined],
  ['Rate', 'kbit/s', '1.1', 1100, (c) => must(c.limiter).rate_bps, undefined],
  ['Rate', 'Mbit/s', '1.5', 1500000, (c) => must(c.limiter).rate_bps, undefined],
  ['Rate', 'Gbit/s', '1.5', 1500000000, (c) => must(c.limiter).rate_bps, undefined],
  ['Rate', 'Gbit/s', '0.067', 67000000, (c) => must(c.limiter).rate_bps, undefined], // 0.067 * 1e9 = 67000000.00000001 in floating point
  ['Rate', 'Gbit/s', '0.267', 267000000, (c) => must(c.limiter).rate_bps, undefined],
  ['Initialization rate', 'pps', '1.5', 1.5, (c) => must(c.delayer).initialization_rate_pps, undefined],
  ['Initialization rate', 'kpps', '1500', 1500000, (c) => must(c.delayer).initialization_rate_pps, undefined],
  ['Initialization rate', 'Mpps', '12.5', 12500000, (c) => must(c.delayer).initialization_rate_pps, undefined],
  ['Initialization rate', 'Mpps', '0.1', 100000, (c) => must(c.delayer).initialization_rate_pps, undefined],
  ['Initialization rate', 'Mpps', '1.1', 1100000, (c) => must(c.delayer).initialization_rate_pps, undefined],
];

describe.each(PAGES)('$name: payload in base units', (page: ConfigPage) => {
  it.each(PAYLOAD)('%s: %s %s is sent as %d', async (label, unit, text, expected, pick, prep) => {
    const user = await page.openFull();
    await page.ready(user);
    if (prep === 'correlation') await user.click(screen.getByLabelText('Activate correlation'));
    await typeInto(user, body(), label, text, unit);
    await page.save(user);
    expect(pick(page.created())).toBe(expected);
  });

  it('every other field keeps its base value when one is edited (units do not leak between fields)', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await typeInto(user, body(), 'Mean delay', '3', 's');
    await page.save(user);
    const c = page.created();
    expect(must(c.delayer).std_ns).toBe(100000);
    expect(must(c.limiter).rate_bps).toBe(1e10);
    expect(must(c.delayer).initialization_rate_pps).toBe(10000000);
  });
});

describe.each(PAGES)('$name: validation per field (messages name the field, never a unit)', (page: ConfigPage) => {
  const NO_UNIT_IN_MESSAGE = /\((ns|us|µs|ms|s|bit\/s|bit|pps)\)/;

  async function ready(user: UserEvent, prep?: Prep) {
    await page.ready(user);
    if (prep === 'correlation') await user.click(screen.getByLabelText('Activate correlation'));
  }

  it.each([
    ['Mean delay', /Mean delay/, undefined],
    ['Delay std', /Delay std/, undefined],
    ['Half-life period', /Half-life period/, 'correlation'],
    ['Rate', /\bRate\b/, undefined],
    ['Initialization rate', /Initialization rate/, undefined],
    ['Buffer capacity', /Buffer capacity/, undefined],
  ] as [string, RegExp, Prep][])(
    'an empty %s is refused: alert naming the field, no request',
    async (label, name, prep) => {
      const user = await page.openFull();
      await ready(user, prep);
      await user.clear(numberField(body(), label));
      await page.save(user);
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(name);
      expect(alert.textContent).not.toMatch(NO_UNIT_IN_MESSAGE);
      expect(page.requests()).toBe(0);
    },
  );

  it.each(['Histogram lo', 'Histogram hi'])('an empty %s is fine (automatic range)', async (label) => {
    const user = await page.openFull();
    await ready(user);
    expect(numberField(body(), label)).toHaveValue(null);
    await page.save(user);
    expect(page.requests()).toBe(1);
  });

  it.each([
    ['0.5', 'Mpps', 500000],
    ['1', 'pps', 1],
    ['0.000001', 'Mpps', 1],
    ['0.001', 'kpps', 1],
    ['2.5', 'kpps', 2500],
  ] as [string, string, number][])(
    'initialization rate %s %s is accepted (>= 1 pps after conversion) and sent as %d',
    async (text, unit, pps) => {
      const user = await page.openFull();
      await ready(user);
      await typeRate(user, text, unit);
      await page.save(user);
      expect(must(page.created().delayer).initialization_rate_pps).toBe(pps);
    },
  );

  it.each([
    ['0.9', 'pps'],
    ['0.0000009', 'Mpps'],
    ['0.0009', 'kpps'],
    ['0', 'Mpps'],
    ['-2', 'kpps'],
  ] as [string, string][])(
    'initialization rate %s %s is below 1 pps: refused with an alert naming the field, no request',
    async (text, unit) => {
      const user = await page.openFull();
      await ready(user);
      await typeRate(user, text, unit);
      await page.save(user);
      const alert = await screen.findByRole('alert');
      expect(alert).toHaveTextContent(/Initialization rate/);
      expect(alert.textContent).not.toMatch(NO_UNIT_IN_MESSAGE);
      expect(page.requests()).toBe(0);
    },
  );

  it('a refused form keeps every typed value and unit and can be corrected and saved', async () => {
    const user = await page.openFull();
    await ready(user);
    await typeInto(user, body(), 'Mean delay', '2.5', MU);
    await typeRate(user, '0.5', 'pps');
    await page.save(user);
    await screen.findByRole('alert');
    expect(numberField(body(), 'Mean delay')).toHaveValue(2.5);
    expect(unitOf(body(), 'Mean delay')).toBe(MU);
    expect(unitOf(body(), 'Initialization rate')).toBe('pps');
    await typeRate(user, '2', 'Mpps');
    await page.save(user);
    const c = page.created();
    expect(must(c.delayer).mean_ns).toBe(2500);
    expect(must(c.delayer).initialization_rate_pps).toBe(2000000);
  });
});

describe.each(PAGES)('$name: stored values of disabled fields survive the unit handling', (page: ConfigPage) => {
  it('v22: a stored limiter is sent back as exactly {mode, rate_bps} in base units (40 Gbit/s)', async () => {
    const user = await page.openStored(cfg({}, { rate_bps: 40000000000 }));
    expect(numberField(body(), 'Rate')).toBeEnabled();
    await page.save(user);
    const l = must(page.updated().limiter);
    expect(l.rate_bps).toBe(40000000000);
    expect(Object.keys(l).sort()).toEqual(['mode', 'rate_bps']);
  });

  it('half-life with correlation off keeps its stored value (2.5 us)', async () => {
    const user = await page.openStored(cfg({ activate_correlation: false, half_life_period_ns: 2500 }));
    expect(numberField(body(), 'Half-life period')).toBeDisabled();
    expect(numberField(body(), 'Half-life period')).toHaveValue(2.5);
    await page.save(user);
    expect(must(page.updated().delayer).half_life_period_ns).toBe(2500);
  });
});
