import { screen, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { linkPage } from '../test/configPages';
import { numberField, section, typeInto, unitOf } from '../test/helpers';
import { must } from '../test/must';

// Power-of-two suggestions in the Statistics section (Histogram bins, lo, hi), look and feel only: advisory, nothing is refused.
// Model: P4TG HistogramModal.tsx. The texts:
//   bins set:   "Power-of-two bin count."  |  "Tip: use a power-of-two bin count (1, 2, 4, …, 4096)."
//   aligned:    "Binary-aligned bins: <width in the hi unit> each (2^k ns)."   (bi-check-circle-fill)
//   otherwise, hi set:  "Suggested range: 0 – <nextPowerOfTwo(hi ns) in the hi unit> (2^k ns)."  (bi-exclamation-circle-fill)
//                       plus a "Use suggestion" button that sets lo = 0 and hi = that value
// Choices:
//   - "(2^k ns)" is literal text "2^6 ns" (no <sup>); textContent is compared
//   - the width and the suggestion are shown in the hi field's CURRENT unit, without trailing zeros (4.096, 0.064, 8192)
//   - "Use suggestion" keeps the hi unit and writes the value in it; lo becomes 0 in lo's own unit
//   - hi = 0 (or an empty hi) gives no range hint at all (there is nothing to round up)
//   - the icons carry the P4TG classes histogram-optimization-icon-aligned / -icon-suggestion, the button
//     histogram-optimization-action
//   - the bins hint appears next to the Histogram bins field (inside the Statistics section)
// The cap of 4096 bins itself is the controller's.

const stats = () => section('Statistics');
const text = () => (stats().textContent ?? '').replace(/\s+/g, ' ');
const BINS_POW2 = 'Power-of-two bin count.';
const BINS_TIP = 'Tip: use a power-of-two bin count (1, 2, 4, …, 4096).';
const useSuggestion = () => within(stats()).queryByRole('button', { name: 'Use suggestion' });

const open = () => linkPage.openNew();
async function bins(user: UserEvent, v: string) {
  const el = numberField(stats(), 'Histogram bins');
  await user.clear(el);
  if (v !== '') await user.type(el, v);
}
const lo = (user: UserEvent, v: string, unit?: string) => typeInto(user, stats(), 'Histogram lo', v, unit);
const hi = (user: UserEvent, v: string, unit?: string) => typeInto(user, stats(), 'Histogram hi', v, unit);

describe('histogram bins: 4096 instead of 500 (spec 30 requirement 4)', () => {
  it('the hint under the field says "Max 4096" and no longer mentions 500', async () => {
    await open();
    expect(text()).toContain('Max 4096');
    expect(text()).not.toMatch(/\b500\b/);
  });

  it('the Histogram bins tooltip says 4096 and not 500', async () => {
    const user = await open();
    await user.hover(screen.getByRole('button', { name: 'About Histogram bins' }));
    const tip = (await screen.findByRole('tooltip')).textContent ?? '';
    expect(tip).toContain('4096');
    expect(tip).not.toMatch(/\b500\b/);
    expect(tip).toMatch(/1 to 4096/);
  });

  it('nothing refuses 4096 bins: the form saves it as typed', async () => {
    const user = await open();
    await linkPage.ready(user);
    await bins(user, '4096');
    await linkPage.save(user);
    expect(linkPage.created().histogram?.bins).toBe(4096);
  });
});

describe('histogram bins: power-of-two hint', () => {
  it('is absent while the field is empty', async () => {
    await open();
    expect(text()).not.toContain(BINS_POW2);
    expect(text()).not.toContain('Tip: use a power-of-two bin count');
  });

  it.each(['1', '2', '64', '256', '4096'])('"%s" is a power of two: "Power-of-two bin count."', async (v) => {
    const user = await open();
    await bins(user, v);
    expect(text()).toContain(BINS_POW2);
    expect(text()).not.toContain('Tip:');
  });

  it.each(['3', '100', '500', '1000', '4095'])('"%s" is not: the tip with the examples and 4096', async (v) => {
    const user = await open();
    await bins(user, v);
    expect(text()).toContain(BINS_TIP);
    expect(text()).not.toContain(BINS_POW2);
  });

  it('follows the typing and disappears when the field is cleared', async () => {
    const user = await open();
    await bins(user, '100');
    expect(text()).toContain(BINS_TIP);
    await bins(user, '128');
    expect(text()).toContain(BINS_POW2);
    expect(text()).not.toContain('Tip:');
    await bins(user, '');
    expect(text()).not.toContain(BINS_POW2);
    expect(text()).not.toContain('Tip:');
  });

  it('is advisory: a non-power-of-two count with a non-aligned range is saved unchanged', async () => {
    const user = await open();
    await linkPage.ready(user);
    await bins(user, '100');
    await lo(user, '1', 'ms');
    await hi(user, '7', 'ms');
    expect(text()).toContain(BINS_TIP);
    await linkPage.save(user);
    expect(linkPage.created().histogram).toEqual({ bins: 100, lo_ns: 1000000, hi_ns: 7000000 });
  });
});

describe('histogram range: binary-aligned check and suggested range', () => {
  it('no range hint while hi is empty (also with bins and lo set)', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '0', 'ns');
    expect(text()).not.toContain('Suggested range');
    expect(text()).not.toContain('Binary-aligned');
    expect(useSuggestion()).not.toBeInTheDocument();
    expect(stats().querySelector('.bi-check-circle-fill, .bi-exclamation-circle-fill')).toBeNull();
  });

  it('hi alone: "Suggested range: 0 – 4.096 µs (2^12 ns)." for 3 µs, with the amber icon and a button', async () => {
    const user = await open();
    await hi(user, '3', 'µs');
    expect(text()).toContain('Suggested range: 0 – 4.096 µs (2^12 ns).');
    const icon = must(stats().querySelector('.bi-exclamation-circle-fill'), 'amber icon');
    expect(icon).toHaveAttribute('aria-hidden', 'true');
    expect(icon).toHaveClass('histogram-optimization-icon-suggestion');
    expect(stats().querySelector('.bi-check-circle-fill')).toBeNull();
    const btn = must(useSuggestion(), 'Use suggestion');
    expect(btn).toHaveAttribute('type', 'button');
    expect(btn).toHaveClass('histogram-optimization-action');
  });

  it.each([
    ['7', 'ms', '0 – 8.388608 ms (2^23 ns)'],
    ['4096', 'ns', '0 – 4096 ns (2^12 ns)'],
    ['4097', 'ns', '0 – 8192 ns (2^13 ns)'],
    ['1', 's', '0 – 1.073741824 s (2^30 ns)'],
    ['100', 'ns', '0 – 128 ns (2^7 ns)'],
  ])('suggestion for hi = %s %s reads "%s" in the same unit', async (value, unit, shown) => {
    const user = await open();
    await hi(user, value, unit);
    expect(text()).toContain(`Suggested range: ${shown}.`);
  });

  it('aligned: lo, hi and bins set with a power-of-two width that divides lo gives the green check', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '0', 'ns');
    await hi(user, '4096', 'ns');
    expect(text()).toContain('Binary-aligned bins: 64 ns each (2^6 ns).');
    const icon = must(stats().querySelector('.bi-check-circle-fill'), 'green icon');
    expect(icon).toHaveAttribute('aria-hidden', 'true');
    expect(icon).toHaveClass('histogram-optimization-icon-aligned');
    expect(stats().querySelector('.bi-exclamation-circle-fill')).toBeNull();
    expect(text()).not.toContain('Suggested range');
    expect(useSuggestion()).not.toBeInTheDocument();
  });

  it('the bin width is shown in the hi field unit: 4.096 µs / 64 = "0.064 µs each (2^6 ns)"', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '0', 'ns');
    await hi(user, '4.096', 'µs');
    expect(text()).toContain('Binary-aligned bins: 0.064 µs each (2^6 ns).');
  });

  it('a non-zero lo that is a multiple of the width stays aligned (lo 1024 ns, hi 5120 ns, 64 bins: width 64 ns)', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '1024', 'ns');
    await hi(user, '5120', 'ns');
    expect(text()).toContain('Binary-aligned bins: 64 ns each (2^6 ns).');
  });

  it('a width of 1 ns is 2^0: aligned', async () => {
    const user = await open();
    await bins(user, '8');
    await lo(user, '0', 'ns');
    await hi(user, '8', 'ns');
    expect(text()).toContain('Binary-aligned bins: 1 ns each (2^0 ns).');
  });

  it('a power-of-two width with lo not a multiple of it is NOT aligned: suggestion for hi', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '100', 'ns');
    await hi(user, '4196', 'ns'); // width 64 ns, but 100 is no multiple of 64
    expect(text()).not.toContain('Binary-aligned');
    expect(text()).toContain('Suggested range: 0 – 8192 ns (2^13 ns).');
    expect(stats().querySelector('.bi-check-circle-fill')).toBeNull();
    expect(useSuggestion()).toBeInTheDocument();
  });

  it('a width that is not a power of two is NOT aligned (lo 1 ms, hi 7 ms, 100 bins: 60 µs)', async () => {
    const user = await open();
    await bins(user, '100');
    await lo(user, '1', 'ms');
    await hi(user, '7', 'ms');
    expect(text()).not.toContain('Binary-aligned');
    expect(text()).toContain('Suggested range: 0 – 8.388608 ms (2^23 ns).');
  });

  it('with bins or lo missing the check cannot apply: only the suggestion is shown', async () => {
    const user = await open();
    await lo(user, '0', 'ns');
    await hi(user, '4096', 'ns');
    expect(text()).not.toContain('Binary-aligned');
    // lo is 0 and hi already is that value, so applying the suggestion would change nothing and it is not shown
    // (histogramHintsSpec28b.test.tsx)
    expect(text()).not.toContain('Suggested range');
    expect(useSuggestion()).not.toBeInTheDocument();
    await bins(user, '64'); // all three are set and aligned
    expect(text()).toContain('Binary-aligned bins: 64 ns each (2^6 ns).');
    await lo(user, '', undefined); // lo cleared again
    expect(text()).not.toContain('Binary-aligned');
    expect(text()).toContain('Suggested range: 0 – 4096 ns (2^12 ns).');
  });

  it('hi = 0 has nothing to suggest: no range hint', async () => {
    const user = await open();
    await hi(user, '0', 'ns');
    expect(text()).not.toContain('Suggested range');
    expect(useSuggestion()).not.toBeInTheDocument();
  });

  it('follows the unit select: converting hi to another unit keeps the same ns and so the same hint', async () => {
    const user = await open();
    await hi(user, '3000', 'ns');
    expect(text()).toContain('Suggested range: 0 – 4096 ns (2^12 ns).');
    const sel = within(stats()).getByLabelText('Histogram hi unit');
    await user.selectOptions(sel, within(sel).getByRole('option', { name: 'µs' }));
    expect(text()).toContain('Suggested range: 0 – 4.096 µs (2^12 ns).');
  });

  it('the hint is outside the labels: every field keeps its plain accessible name', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '0', 'ns');
    await hi(user, '3', 'µs');
    for (const l of ['Histogram bins', 'Histogram lo', 'Histogram hi']) {
      expect(within(stats()).getByLabelText(l).tagName).toBe('INPUT');
    }
    expect(within(stats()).getByLabelText('Histogram hi unit').tagName).toBe('SELECT');
  });
});

describe('"Use suggestion" (sets lo = 0 and hi = the suggested value)', () => {
  it('writes hi in its own unit and lo = 0, then the hint is gone once bins make it aligned', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '1', 'µs');
    await hi(user, '3', 'µs'); // width 31.25 ns: not aligned
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(numberField(stats(), 'Histogram hi')).toHaveValue(4.096);
    expect(unitOf(stats(), 'Histogram hi')).toBe('µs');
    expect(numberField(stats(), 'Histogram lo')).toHaveValue(0);
    expect(text()).toContain('Binary-aligned bins: 0.064 µs each (2^6 ns).');
    expect(text()).not.toContain('Suggested range');
    expect(useSuggestion()).not.toBeInTheDocument();
  });

  it('works with only hi typed (lo and bins empty): lo becomes 0 and hi the next power of two', async () => {
    const user = await open();
    await hi(user, '7', 'ms');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(numberField(stats(), 'Histogram hi')).toHaveValue(8.388608);
    expect(unitOf(stats(), 'Histogram hi')).toBe('ms');
    expect(numberField(stats(), 'Histogram lo')).toHaveValue(0);
  });

  it('the suggested values are what gets saved: lo_ns 0 and hi_ns the power of two', async () => {
    const user = await open();
    await linkPage.ready(user);
    await bins(user, '64');
    await hi(user, '3', 'µs');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    await linkPage.save(user);
    expect(linkPage.created().histogram).toEqual({ bins: 64, lo_ns: 0, hi_ns: 4096 });
  });

  it('does not touch the bin count', async () => {
    const user = await open();
    await bins(user, '100');
    await hi(user, '7', 'ms');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(numberField(stats(), 'Histogram bins')).toHaveValue(100);
    expect(text()).toContain(BINS_TIP);
  });

  it('is a plain text button: not inside a label or a legend, not a submit', async () => {
    const user = await open();
    await hi(user, '3', 'µs');
    const btn = must(useSuggestion(), 'Use suggestion');
    expect(btn.closest('label')).toBeNull();
    expect(btn.closest('legend')).toBeNull();
    expect(btn).toHaveAttribute('type', 'button');
  });

  it('clicking it saves nothing by itself', async () => {
    const user = await open();
    await hi(user, '3', 'µs');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(linkPage.requests()).toBe(0);
  });
});
