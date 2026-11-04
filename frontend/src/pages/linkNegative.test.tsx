import { screen } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import { numberField, typeInto, unitOf } from '../test/helpers';
import { must } from '../test/must';

// spec19 item 6. The form refuses negative values for times, rates, capacity and the initialization rate, and loss
// probabilities outside [0, 1], in the unit the user typed. The message names the field (like the other validation
// messages) and says why; it never quotes the base-unit value (a user who typed "-5 ms" must not read "-5000000").
// The controller stays the final check; these tests only pin the frontend's own refusal (no request is made).
// Every case runs against the link editor (the only page with the config form since spec27).
//
// Deliberately NOT pinned: whether 0 is refused for Rate (the controller wants > 0 while the limiter is active) and
// what a negative value in a field that is switched off (half-life with correlation off) does. v22: the limiter has no switch.
const MU = 'µs';
const body = () => document.body;

/** The reason in words, whatever the exact phrasing: "must not be negative", "at least 0", ... */
const NEGATIVE_REASON = /negative|0 or more|at least 0|≥ ?0|>= ?0|less than 0|below 0/i;
/** The range in words or brackets: "between 0 and 1", "[0, 1]". */
const RANGE_REASON = /between 0 and 1|0 and 1|\[0, ?1\]|0 to 1|0–1/i;

type Prep = 'correlation' | undefined;

// label, text, unit (undefined = no unit select), prep, the base-unit number as the API would carry it
const NEGATIVES: [string, string, string | undefined, Prep, number][] = [
  ['Mean delay', '-5', 'ms', undefined, -5000000],
  ['Mean delay', '-0.5', MU, undefined, -500],
  ['Mean delay', '-3', 'ns', undefined, -3],
  ['Mean delay', '-1', 's', undefined, -1000000000],
  ['Delay std', '-1', MU, undefined, -1000],
  ['Delay std', '-2', 'ms', undefined, -2000000],
  ['Half-life period', '-2', 'ms', 'correlation', -2000000],
  ['Histogram lo', '-1', 'ms', undefined, -1000000],
  ['Histogram hi', '-3', MU, undefined, -3000],
  ['Rate', '-1', 'Gbit/s', undefined, -1000000000],
  ['Rate', '-250', 'kbit/s', undefined, -250000],
  ['Initialization rate', '-2', 'Mpps', undefined, -2000000],
  ['Initialization rate', '-5', 'kpps', undefined, -5000],
  ['Buffer capacity', '-5', undefined, undefined, -5],
];

/** The digits of the base-unit value without its sign: "5000000". Short values such as "3" or "5" are not searched for. */
const baseDigits = (n: number) => String(Math.abs(n));

describe.each(PAGES)('$name: negative values are refused in the unit the user typed', (page: ConfigPage) => {
  async function ready(user: UserEvent, prep: Prep) {
    await page.ready(user);
    if (prep === 'correlation') await user.click(screen.getByLabelText('Activate correlation'));
  }

  it.each(NEGATIVES)(
    '%s = %s %s is refused: alert naming the field and the reason, no request',
    async (label, text, unit, prep, base) => {
      const user = await page.openFull();
      await ready(user, prep);
      await typeInto(user, body(), label, text, unit);
      await page.save(user);
      const alert = await screen.findByRole('alert');
      // the label as a whole word (case-sensitive, so "Rate" is not satisfied by "Initialization rate")
      expect(alert.textContent).toMatch(new RegExp(`\\b${label}\\b`));
      // the initialization rate already says "must be at least 1 pps", which is the reason in its own words
      expect(alert.textContent).toMatch(label === 'Initialization rate' ? /at least 1 pps/ : NEGATIVE_REASON);
      if (Math.abs(base) >= 100) expect(alert.textContent).not.toContain(baseDigits(base));
      expect(alert.textContent).not.toMatch(/\((ns|us|µs|ms|s|bit\/s|bit|pps)\)/);
      expect(page.requests()).toBe(0);
      // the typed value and unit stay for the correction
      expect(numberField(body(), label)).toHaveValue(Number(text));
      if (unit !== undefined) expect(unitOf(body(), label)).toBe(unit);
    },
  );

  it('a negative buffer capacity is refused on a form with no stage added (alert naming the field, no write)', async () => {
    const user = await page.openNew();
    await page.ready(user);
    await typeInto(user, body(), 'Buffer capacity', '-5');
    await page.save(user);
    expect((await screen.findByRole('alert')).textContent).toMatch(/Buffer capacity/);
    expect(page.requests()).toBe(0);
  });

  it('a refused negative mean delay can be corrected and saved: -5 ms, then 5 ms', async () => {
    const user = await page.openFull();
    await ready(user, undefined);
    await typeInto(user, body(), 'Mean delay', '-5', 'ms');
    await page.save(user);
    await screen.findByRole('alert');
    expect(page.requests()).toBe(0);
    await typeInto(user, body(), 'Mean delay', '5');
    await page.save(user);
    expect(must(page.created().delayer).mean_ns).toBe(5000000);
  });

  it.each([
    ['mean_ns', { mean_ns: -5000000 }, /Mean delay/],
    ['std_ns', { std_ns: -1000 }, /Delay std/],
  ] as const)('a stored negative %s (hand-edited state) is refused on Save as well', async (_name, delayer, name) => {
    const user = await page.openStored(cfg(delayer));
    await page.save(user);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(name);
    expect(page.requests()).toBe(0);
  });

  it('a stored negative rate of the added limiter is refused on Save', async () => {
    const user = await page.openStored(cfg({}, { rate_bps: -1e9 }));
    await page.save(user);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/\bRate\b/);
    expect(page.requests()).toBe(0);
  });

  // The boundary: zero is not negative (a deterministic zero delay, no spread and an unbounded-by-count buffer are valid).
  it.each([
    ['Mean delay', 'mean_ns'],
    ['Delay std', 'std_ns'],
  ] as const)('%s = 0 is accepted and sent as 0', async (label, key) => {
    const user = await page.openFull();
    await ready(user, undefined);
    await typeInto(user, body(), label, '0', 'ms');
    await page.save(user);
    expect(must(page.created().delayer)[key]).toBe(0);
  });

  it('Buffer capacity = 0 is accepted and sent as 0 (also with no stage added)', async () => {
    const user = await page.openNew();
    await ready(user, undefined);
    await typeInto(user, body(), 'Buffer capacity', '0');
    await page.save(user);
    expect(page.created().buffer_capacity).toBe(0);
  });

  it('a positive value of every checked field still goes through, in base units', async () => {
    const user = await page.openFull();
    await ready(user, 'correlation');
    await typeInto(user, body(), 'Mean delay', '5', 'ms');
    await typeInto(user, body(), 'Delay std', '0.5', MU);
    await typeInto(user, body(), 'Half-life period', '2', 'ms');
    await typeInto(user, body(), 'Rate', '1', 'Gbit/s');
    await typeInto(user, body(), 'Histogram lo', '1', 'ms');
    await typeInto(user, body(), 'Histogram hi', '9', 'ms');
    await typeRateOk(user);
    await page.save(user);
    const c = page.created();
    expect(must(c.delayer).mean_ns).toBe(5000000);
    expect(must(c.delayer).std_ns).toBe(500);
    expect(must(c.delayer).half_life_period_ns).toBe(2000000);
    expect(must(c.limiter).rate_bps).toBe(1000000000);
    expect(c.histogram?.lo_ns).toBe(1000000);
    expect(c.histogram?.hi_ns).toBe(9000000);
  });
});

async function typeRateOk(user: UserEvent) {
  await typeInto(user, body(), 'Initialization rate', '2', 'Mpps');
}

// ---- loss probabilities ---------------------------------------------------------------------------------------------

const GE_FIELDS = ['Gilbert-Elliot p', 'Gilbert-Elliot r', 'Gilbert-Elliot k', 'Gilbert-Elliot h'] as const;

describe.each(PAGES)('$name: loss probabilities must lie within [0, 1]', (page: ConfigPage) => {
  async function ready(user: UserEvent, model: 'Random' | 'GilbertElliot') {
    await page.ready(user);
    if (model === 'GilbertElliot') await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
  }

  it.each(['-0.1', '-1', '1.5', '2', '1.0001'])(
    'Random loss probability %s is refused: alert naming the field and the range, no request',
    async (text) => {
      const user = await page.openFull();
      await ready(user, 'Random');
      await typeInto(user, body(), 'Random loss probability', text);
      await page.save(user);
      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toMatch(/Random loss probability/);
      expect(alert.textContent).toMatch(RANGE_REASON);
      expect(page.requests()).toBe(0);
    },
  );

  it.each(GE_FIELDS.flatMap((f) => ['-0.1', '1.5'].map((v) => [f, v] as const)))(
    '%s = %s is refused: alert naming the field and the range, no request',
    async (label, text) => {
      const user = await page.openFull();
      await ready(user, 'GilbertElliot');
      await typeInto(user, body(), label, text);
      await page.save(user);
      const alert = await screen.findByRole('alert');
      expect(alert.textContent).toContain(label);
      expect(alert.textContent).toMatch(RANGE_REASON);
      expect(page.requests()).toBe(0);
    },
  );

  it.each(['0', '1', '0.5', '0.000001', '0.999999'])(
    'Random loss probability %s is accepted and sent as typed',
    async (text) => {
      const user = await page.openFull();
      await ready(user, 'Random');
      await typeInto(user, body(), 'Random loss probability', text);
      await page.save(user);
      expect(page.created().loss_model).toEqual({ type: 'Random', p: Number(text) });
    },
  );

  it.each(GE_FIELDS.flatMap((f) => ['0', '1'].map((v) => [f, v] as const)))(
    '%s = %s (the boundary) is accepted',
    async (label, text) => {
      const user = await page.openFull();
      await ready(user, 'GilbertElliot');
      await typeInto(user, body(), label, text);
      await page.save(user);
      const loss = must(page.created().loss_model);
      expect(loss.type).toBe('GilbertElliot');
      const key = label.slice(-1) as 'p' | 'r' | 'k' | 'h';
      expect((loss as unknown as Record<string, number>)[key]).toBe(Number(text));
    },
  );

  it('a stored probability outside [0, 1] is refused on Save as well', async () => {
    const base = cfg();
    const user = await page.openStored({ ...base, loss_model: { type: 'Random', p: 1.5 } });
    await page.save(user);
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toMatch(/Random loss probability/);
    expect(page.requests()).toBe(0);
  });

  it('only the chosen model is checked: an out-of-range value in the hidden Random field does not block Gilbert-Elliot', async () => {
    const user = await page.openFull();
    await ready(user, 'Random');
    await typeInto(user, body(), 'Random loss probability', '5');
    await user.selectOptions(screen.getByLabelText('Loss model'), 'GilbertElliot');
    await page.save(user);
    expect(must(page.created().loss_model).type).toBe('GilbertElliot');
  });

  it('a refused probability can be corrected and saved: 1.5, then 0.5', async () => {
    const user = await page.openFull();
    await ready(user, 'Random');
    await typeInto(user, body(), 'Random loss probability', '1.5');
    await page.save(user);
    await screen.findByRole('alert');
    expect(page.requests()).toBe(0);
    await typeInto(user, body(), 'Random loss probability', '0.5');
    await page.save(user);
    expect(page.created().loss_model).toEqual({ type: 'Random', p: 0.5 });
  });
});
