import { within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { linkPage } from '../test/configPages';
import { numberField, section, typeInto, unitOf } from '../test/helpers';
import { must } from '../test/must';

// Spec 28b item 6 (spec: scratchpad/spec28b.md): two fixes to the histogram hints of histogramHints.test.tsx.
//   a. The suggestion is not shown when lo is 0 and hi already equals the suggested value (applying it would change
//      nothing). CHOICES: "lo is 0" means a typed 0, in any unit; an EMPTY lo is not 0 (the suggestion would still fill it in,
//      so it stays); the comparison is in ns, so 4.096 µs equals 4096 ns; when the suggestion is not shown and the bins are
//      not aligned either, no hint and no button appear at all.
//   b. A power-of-two bin width below 1 ns (2^-k ns) counts as binary-aligned and is shown as "(2^-2 ns)", with the width in
//      the hi field's unit like the others ("0.25 ns each", "0.00025 µs each"). lo and hi are whole ns, so such a width
//      needs more bins than ns in the range.
// The changed expectation in histogramHints.test.tsx is its test "with bins or lo missing the check cannot apply".

const stats = () => section('Statistics');
const text = () => (stats().textContent ?? '').replace(/\s+/g, ' ');
const useSuggestion = () => within(stats()).queryByRole('button', { name: 'Use suggestion' });
const noRangeHint = () => {
  expect(text()).not.toContain('Suggested range');
  expect(text()).not.toContain('Binary-aligned');
  expect(useSuggestion()).not.toBeInTheDocument();
  expect(stats().querySelector('.bi-check-circle-fill, .bi-exclamation-circle-fill')).toBeNull();
};

const open = () => linkPage.openNew();
async function bins(user: UserEvent, v: string) {
  const el = numberField(stats(), 'Histogram bins');
  await user.clear(el);
  if (v !== '') await user.type(el, v);
}
const lo = (user: UserEvent, v: string, unit?: string) => typeInto(user, stats(), 'Histogram lo', v, unit);
const hi = (user: UserEvent, v: string, unit?: string) => typeInto(user, stats(), 'Histogram hi', v, unit);

describe('histogram hints (spec 28b item 6a): no suggestion that changes nothing', () => {
  it('lo 0 and hi already a power of two (4096 ns), bins empty: no suggestion, no button, no icon', async () => {
    const user = await open();
    await lo(user, '0', 'ns');
    await hi(user, '4096', 'ns');
    noRangeHint();
  });

  it('the same with bins that do not make the range aligned (100 bins): the bins tip stays, nothing else', async () => {
    const user = await open();
    await bins(user, '100');
    await lo(user, '0', 'ns');
    await hi(user, '4096', 'ns');
    noRangeHint();
    expect(text()).toContain('Tip: use a power-of-two bin count (1, 2, 4, …, 4096).');
  });

  it('the comparison is in ns: lo 0 µs and hi 4.096 µs is the same range', async () => {
    const user = await open();
    await lo(user, '0', 'µs');
    await hi(user, '4.096', 'µs');
    noRangeHint();
  });

  it('lo typed in another unit but 0 counts as 0', async () => {
    const user = await open();
    await lo(user, '0', 'ms');
    await hi(user, '1', 's'); // 1e9 ns is not a power of two: this one still gets its suggestion
    expect(text()).toContain('Suggested range: 0 – 1.073741824 s (2^30 ns).');
    await hi(user, '1.073741824', 's'); // 2^30 ns exactly
    noRangeHint();
  });

  it('with the range already aligned the green check is still shown (it is not a suggestion)', async () => {
    const user = await open();
    await bins(user, '64');
    await lo(user, '0', 'ns');
    await hi(user, '4096', 'ns');
    expect(text()).toContain('Binary-aligned bins: 64 ns each (2^6 ns).');
    expect(useSuggestion()).not.toBeInTheDocument();
  });

  it('a lo that is not 0 keeps the suggestion even when hi is a power of two (it would set lo to 0)', async () => {
    const user = await open();
    await lo(user, '1', 'ns');
    await hi(user, '4096', 'ns');
    expect(text()).toContain('Suggested range: 0 – 4096 ns (2^12 ns).');
    expect(useSuggestion()).toBeInTheDocument();
  });

  it('hi not a power of two keeps it, also with lo 0', async () => {
    const user = await open();
    await lo(user, '0', 'ns');
    await hi(user, '4097', 'ns');
    expect(text()).toContain('Suggested range: 0 – 8192 ns (2^13 ns).');
    expect(useSuggestion()).toBeInTheDocument();
  });

  it('an empty lo is not 0: the suggestion stays and fills lo in', async () => {
    const user = await open();
    await hi(user, '4096', 'ns');
    expect(text()).toContain('Suggested range: 0 – 4096 ns (2^12 ns).');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(numberField(stats(), 'Histogram lo')).toHaveValue(0);
    expect(numberField(stats(), 'Histogram hi')).toHaveValue(4096);
    noRangeHint(); // now lo is 0 and hi the suggested value
  });

  it('after "Use suggestion" with bins that do not align, the suggestion is gone (it would change nothing now)', async () => {
    const user = await open();
    await bins(user, '100');
    await lo(user, '1', 'ms');
    await hi(user, '7', 'ms');
    await user.click(must(useSuggestion(), 'Use suggestion'));
    expect(numberField(stats(), 'Histogram hi')).toHaveValue(8.388608);
    noRangeHint();
  });

  it('typing hi up to the suggested value makes the suggestion disappear, typing past it brings the next one', async () => {
    const user = await open();
    await lo(user, '0', 'ns');
    await hi(user, '4000', 'ns');
    expect(text()).toContain('Suggested range: 0 – 4096 ns (2^12 ns).');
    await hi(user, '4096', 'ns');
    noRangeHint();
    await hi(user, '4097', 'ns');
    expect(text()).toContain('Suggested range: 0 – 8192 ns (2^13 ns).');
  });
});

describe('histogram hints (spec 28b item 6b): widths below 1 ns are binary-aligned too', () => {
  it('4096 bins over 0 to 1024 ns: width 0.25 ns = 2^-2 ns', async () => {
    const user = await open();
    await bins(user, '4096');
    await lo(user, '0', 'ns');
    await hi(user, '1024', 'ns');
    expect(text()).toContain('Binary-aligned bins: 0.25 ns each (2^-2 ns).');
    expect(text()).not.toContain('Suggested range');
    expect(useSuggestion()).not.toBeInTheDocument();
    const icon = must(stats().querySelector('.bi-check-circle-fill'), 'green icon');
    expect(icon).toHaveClass('histogram-optimization-icon-aligned');
    expect(stats().querySelector('.bi-exclamation-circle-fill')).toBeNull();
  });

  it.each([
    ['2048', '1024', '0.5 ns each (2^-1 ns)'],
    ['4096', '1024', '0.25 ns each (2^-2 ns)'],
    ['1024', '1', '0.0009765625 ns each (2^-10 ns)'],
    ['8', '4', '0.5 ns each (2^-1 ns)'],
  ])('%s bins over 0 to %s ns: "%s"', async (nBins, hiNs, shown) => {
    const user = await open();
    await bins(user, nBins);
    await lo(user, '0', 'ns');
    await hi(user, hiNs, 'ns');
    expect(text()).toContain(`Binary-aligned bins: ${shown}.`);
  });

  it('the width is shown in the hi unit: 4096 bins over 0 to 1.024 µs is "0.00025 µs each (2^-2 ns)"', async () => {
    const user = await open();
    await bins(user, '4096');
    await lo(user, '0', 'ns');
    await hi(user, '1.024', 'µs');
    expect(text()).toContain('Binary-aligned bins: 0.00025 µs each (2^-2 ns).');
  });

  it('a non-zero whole-ns lo is a multiple of such a width: lo 1 ns, hi 1025 ns, 4096 bins', async () => {
    const user = await open();
    await bins(user, '4096');
    await lo(user, '1', 'ns');
    await hi(user, '1025', 'ns');
    expect(text()).toContain('Binary-aligned bins: 0.25 ns each (2^-2 ns).');
  });

  it('a sub-ns width that is not a power of two is not aligned (3000 bins over 1 to 1024 ns): the suggestion', async () => {
    const user = await open();
    await bins(user, '3000');
    await lo(user, '1', 'ns');
    await hi(user, '1024', 'ns');
    expect(text()).not.toContain('Binary-aligned');
    expect(text()).toContain('Suggested range: 0 – 1024 ns (2^10 ns).');
  });

  it('the widths of 1 ns and more are as before (2^0, 2^6)', async () => {
    const user = await open();
    await bins(user, '8');
    await lo(user, '0', 'ns');
    await hi(user, '8', 'ns');
    expect(text()).toContain('Binary-aligned bins: 1 ns each (2^0 ns).');
    await bins(user, '64');
    await hi(user, '4096', 'ns');
    expect(text()).toContain('Binary-aligned bins: 64 ns each (2^6 ns).');
  });

  it('is advisory: nothing is refused, the range is saved as typed', async () => {
    const user = await open();
    await linkPage.ready(user);
    await bins(user, '4096');
    await lo(user, '0', 'ns');
    await hi(user, '1024', 'ns');
    await linkPage.save(user);
    expect(linkPage.created().histogram).toEqual({ bins: 4096, lo_ns: 0, hi_ns: 1024 });
  });

  it('the unit select still converts: the same sub-ns width in ns after switching hi to ns', async () => {
    const user = await open();
    await bins(user, '4096');
    await lo(user, '0', 'ns');
    await hi(user, '1.024', 'µs');
    expect(text()).toContain('0.00025 µs each (2^-2 ns)');
    const sel = within(stats()).getByLabelText('Histogram hi unit');
    await user.selectOptions(sel, within(sel).getByRole('option', { name: 'ns' }));
    expect(unitOf(stats(), 'Histogram hi')).toBe('ns');
    expect(text()).toContain('Binary-aligned bins: 0.25 ns each (2^-2 ns).');
  });
});
