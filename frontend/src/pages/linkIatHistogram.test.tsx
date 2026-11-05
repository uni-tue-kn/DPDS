import { screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import type { ConfigDto } from '../api/types';
import { linkPage } from '../test/configPages';
import * as fx from '../test/fixtures';
import { must } from '../test/must';
import { expectOptional, numberField, section, TIME_UNITS, typeInto, unitOf, unitSelect } from '../test/helpers';

// Spec 53 A (user 2026-11-05: "IAT histogram should be configurable as well"). The link config gets an optional
// `iat_histogram` {bins, lo_ns, hi_ns} of the delay histogram's shape. In the Statistics section, next to the delay histogram
// fields: "IAT histogram bins" (no unit), "IAT histogram lo" and "IAT histogram hi" (time units, default ms), empty = default
// (256 bins, lo 0, hi 4 x 1e9 / initialization rate), enabled exactly when the delay histogram fields are. Empty fields are
// not sent; the validation messages name "IAT histogram" (not "Histogram", which is the delay histogram).
// Choices where the spec is silent: only the checks the delay histogram form has are pinned here (a non-integer bins value, a
// negative lo / hi; the range rules lo < hi and 1..=4096 bins are the controller's); "null or absent" are the same optional value.
const BINS = 'IAT histogram bins';
const LO = 'IAT histogram lo';
const HI = 'IAT histogram hi';
const MU = 'µs';
const stats = () => section('Statistics');
const about = (label: string) => screen.getByRole('button', { name: `About ${label}` });

/** Delay and IAT histogram both set, different from each other in every number; delay statistics off (fx.config2()). */
const BOTH: ConfigDto = {
  ...fx.config2(),
  histogram: { bins: 32, lo_ns: 1000, hi_ns: 50000 },
  iat_histogram: { bins: 64, lo_ns: 2000, hi_ns: 80000 },
};
const iatOf = (c: ConfigDto) => c.iat_histogram;

async function savedConfig(user: UserEvent, fill: (u: UserEvent) => Promise<void>): Promise<ConfigDto> {
  await linkPage.ready(user);
  await fill(user);
  await linkPage.save(user);
  return linkPage.created();
}

describe('IAT histogram fields: labels, units, place', () => {
  it('three fields in the Statistics section, after the delay histogram fields, empty on a new form', async () => {
    await linkPage.openNew();
    const own = stats();
    const order = ['Histogram hi', BINS, LO, HI].map((l) => numberField(own, l));
    for (let i = 0; i + 1 < order.length; i += 1) {
      expect(order[i].compareDocumentPosition(order[i + 1]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    for (const l of [BINS, LO, HI]) expect(numberField(own, l)).toHaveValue(null);
  });

  it.each([LO, HI])('%s has one time unit select (ns, us, ms, s), shown as ms while empty', async (label) => {
    await linkPage.openNew();
    expect(screen.getAllByLabelText(`${label} unit`)).toHaveLength(1);
    const sel = unitSelect(stats(), label);
    expect(sel.tagName).toBe('SELECT');
    expect(
      within(sel)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(TIME_UNITS);
    expect(unitOf(stats(), label)).toBe('ms');
  });

  it('the bins field has no unit select', async () => {
    await linkPage.openNew();
    expect(screen.queryByLabelText(`${BINS} unit`)).not.toBeInTheDocument();
  });

  it('none of the three is required', async () => {
    await linkPage.openNew();
    for (const l of [BINS, LO, HI]) expectOptional(numberField(stats(), l));
  });

  it('each has a help button saying the default: 256 bins of 1 to 4096, lo 0, hi from the initialization rate', async () => {
    const user = await linkPage.openNew();
    const tip = async (label: string) => {
      await user.hover(about(label));
      const text = (await screen.findByRole('tooltip')).textContent ?? '';
      await user.unhover(about(label));
      return text;
    };
    const bins = await tip(BINS);
    expect(bins).toMatch(/256/);
    expect(bins).toMatch(/1 to 4096/);
    expect(await tip(LO)).toMatch(/\b0\b/);
    expect(await tip(HI)).toMatch(/initialization rate/i);
  });

  it('the delay histogram fields keep their labels (the plain labels still find exactly one field)', async () => {
    await linkPage.openNew();
    for (const l of ['Histogram bins', 'Histogram lo', 'Histogram hi']) {
      expect(screen.getAllByLabelText(l)).toHaveLength(1);
    }
  });
});

describe('IAT histogram fields: enabled exactly when the delay histogram fields are', () => {
  const PAIRS: [string, string][] = [
    [BINS, 'Histogram bins'],
    [LO, 'Histogram lo'],
    [`${LO} unit`, 'Histogram lo unit'],
    [HI, 'Histogram hi'],
    [`${HI} unit`, 'Histogram hi unit'],
  ];
  const same = () => {
    for (const [iat, delay] of PAIRS) {
      const d = within(stats()).getByLabelText(delay) as HTMLInputElement;
      const i = within(stats()).getByLabelText(iat) as HTMLInputElement;
      expect(i.disabled, `${iat} vs ${delay}`).toBe(d.disabled);
    }
  };

  it('with delay statistics on, then off, then on again', async () => {
    const user = await linkPage.openNew();
    same();
    await user.click(within(stats()).getByLabelText('Delay statistics'));
    same();
    await user.click(within(stats()).getByLabelText('Delay statistics'));
    same();
  });

  it('when editing a link whose delay statistics are off', async () => {
    await linkPage.openStored(BOTH);
    expect(within(stats()).getByLabelText('Delay statistics')).not.toBeChecked();
    same();
  });
});

describe('IAT histogram: what is saved', () => {
  it('all three empty: no iat_histogram values are sent, and the delay histogram is untouched', async () => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async () => {});
    expect(iatOf(c) ?? null).toBeNull();
    expect(c.histogram ?? null).toBeNull();
  });

  it('bins, lo in ms and hi in us are sent as typed in base units', async () => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => {
      await typeInto(u, stats(), BINS, '128');
      await typeInto(u, stats(), LO, '1', 'ms');
      await typeInto(u, stats(), HI, '4000', MU);
    });
    expect(iatOf(c)).toEqual({ bins: 128, lo_ns: 1000000, hi_ns: 4000000 });
    expect(c.histogram ?? null).toBeNull();
  });

  it.each([
    [LO, 'ns', '100', 100],
    [LO, 'ms', '1.005', 1005000],
    [HI, 's', '2', 2000000000],
    [HI, MU, '9', 9000],
  ] as [string, string, string, number][])('%s = %s %s is sent as %d', async (label, unit, text, expected) => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => typeInto(u, stats(), label, text, unit));
    const got = label === LO ? iatOf(c)?.lo_ns : iatOf(c)?.hi_ns;
    expect(got).toBe(expected);
  });

  it('an explicit lo of 0 is a value, not "empty": it is sent as 0', async () => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => typeInto(u, stats(), LO, '0'));
    expect(iatOf(c)?.lo_ns).toBe(0);
    expect(iatOf(c)?.bins ?? null).toBeNull();
    expect(iatOf(c)?.hi_ns ?? null).toBeNull();
  });

  it.each([
    ['bins', BINS, '64', 'bins'],
    ['hi', HI, '5', 'hi_ns'],
  ] as const)('only %s typed: the other two stay empty (default)', async (_n, label, text, key) => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => typeInto(u, stats(), label, text, label === HI ? 'ms' : undefined));
    const iat = must(iatOf(c));
    for (const k of ['bins', 'lo_ns', 'hi_ns'] as const) {
      if (k === key) expect(iat[k]).not.toBeNull();
      else expect(iat[k] ?? null).toBeNull();
    }
  });

  it("4096 bins are saved as typed (the cap is the controller's)", async () => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => typeInto(u, stats(), BINS, '4096'));
    expect(iatOf(c)?.bins).toBe(4096);
  });

  it('the two histograms are independent: delay values do not leak into the IAT ones and back', async () => {
    const user = await linkPage.openNew();
    const c = await savedConfig(user, async (u) => {
      await typeInto(u, stats(), 'Histogram bins', '32');
      await typeInto(u, stats(), 'Histogram hi', '50', MU);
      await typeInto(u, stats(), BINS, '64');
      await typeInto(u, stats(), LO, '2', MU);
    });
    expect(c.histogram).toEqual({ bins: 32, lo_ns: null, hi_ns: 50000 });
    expect(iatOf(c)).toEqual({ bins: 64, lo_ns: 2000, hi_ns: null });
  });
});

describe('IAT histogram: validation names "IAT histogram"', () => {
  async function refused(label: string, text: string, unit?: string) {
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await typeInto(user, stats(), label, text, unit);
    await linkPage.save(user);
    const alert = await screen.findByRole('alert');
    expect(linkPage.requests()).toBe(0);
    // the field is named with the IAT prefix, never as the delay histogram's own field
    expect(alert.textContent).toContain(label);
    expect(alert.textContent).not.toMatch(/\bHistogram (bins|lo|hi)\b/);
    return { user, text: alert.textContent ?? '' };
  }

  it('a bins value that is not a whole number is refused, nothing is written', async () => {
    const { text } = await refused(BINS, '1.5');
    expect(text).toMatch(/integer|whole number/i);
  });

  it.each([
    [LO, '-1', 'ms', '1000000'],
    [HI, '-3', MU, '3000'],
  ])('%s = %s %s is refused as negative, without the base-unit number', async (label, text, unit, base) => {
    const { text: msg } = await refused(label, text, unit);
    expect(msg).toMatch(/negative|at least 0/i);
    expect(msg).not.toContain(base);
    expect(numberField(stats(), label)).toHaveValue(Number(text));
    expect(unitOf(stats(), label)).toBe(unit);
  });

  it('after the correction the form saves', async () => {
    const { user } = await refused(BINS, '1.5');
    await typeInto(user, stats(), BINS, '16');
    await linkPage.save(user);
    expect(iatOf(linkPage.created())?.bins).toBe(16);
  });

  it('an invalid DELAY histogram value still names "Histogram lo", not the IAT field', async () => {
    const user = await linkPage.openNew();
    await linkPage.ready(user);
    await typeInto(user, stats(), 'Histogram lo', '-1', 'ms');
    await linkPage.save(user);
    const text = (await screen.findByRole('alert')).textContent ?? '';
    expect(text).toMatch(/\bHistogram lo\b/);
    expect(text).not.toContain('IAT');
  });
});

describe('IAT histogram: loading a stored link', () => {
  it('shows the stored values in display units, next to the (different) delay histogram values', async () => {
    await linkPage.openStored(BOTH);
    expect(numberField(stats(), BINS)).toHaveValue(64);
    expect(numberField(stats(), LO)).toHaveValue(2); // 2000 ns
    expect(unitOf(stats(), LO)).toBe(MU);
    expect(numberField(stats(), HI)).toHaveValue(80); // 80000 ns
    expect(unitOf(stats(), HI)).toBe(MU);
    expect(numberField(stats(), 'Histogram bins')).toHaveValue(32);
    expect(numberField(stats(), 'Histogram hi')).toHaveValue(50);
  });

  it('a link without iat_histogram shows three empty fields', async () => {
    await linkPage.openStored(fx.config2());
    for (const l of [BINS, LO, HI]) expect(numberField(stats(), l)).toHaveValue(null);
    expect(unitOf(stats(), LO)).toBe('ms');
    expect(unitOf(stats(), HI)).toBe('ms');
  });

  it('a stored iat_histogram with only some members fills those and leaves the rest empty', async () => {
    await linkPage.openStored({ ...fx.config2(), iat_histogram: { bins: 16, lo_ns: null, hi_ns: 5000000 } });
    expect(numberField(stats(), BINS)).toHaveValue(16);
    expect(numberField(stats(), LO)).toHaveValue(null);
    expect(numberField(stats(), HI)).toHaveValue(5);
    expect(unitOf(stats(), HI)).toBe('ms');
  });

  it('saving unchanged stores the same iat_histogram and the same delay histogram', async () => {
    const user = await linkPage.openStored(BOTH);
    await linkPage.save(user);
    const c = linkPage.updated();
    expect(iatOf(c)).toEqual({ bins: 64, lo_ns: 2000, hi_ns: 80000 });
    expect(c.histogram).toEqual({ bins: 32, lo_ns: 1000, hi_ns: 50000 });
  });

  it('clearing the three fields sends no IAT values, the delay histogram stays', async () => {
    const user = await linkPage.openStored(BOTH);
    for (const l of [BINS, LO, HI]) await user.clear(numberField(stats(), l));
    await linkPage.save(user);
    const c = linkPage.updated();
    expect(iatOf(c)?.bins ?? null).toBeNull();
    expect(iatOf(c)?.lo_ns ?? null).toBeNull();
    expect(iatOf(c)?.hi_ns ?? null).toBeNull();
    expect(c.histogram).toEqual({ bins: 32, lo_ns: 1000, hi_ns: 50000 });
  });

  it('changing one IAT value changes only that one', async () => {
    const user = await linkPage.openStored(BOTH);
    await typeInto(user, stats(), HI, '1', 's');
    await linkPage.save(user);
    expect(iatOf(linkPage.updated())).toEqual({ bins: 64, lo_ns: 2000, hi_ns: 1000000000 });
  });
});
