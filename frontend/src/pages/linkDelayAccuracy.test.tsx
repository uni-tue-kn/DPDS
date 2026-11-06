import { screen, within } from '@testing-library/react';
import type { ConfigDto } from '../api/types';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import {
  ACCURACY_MODES,
  accuracyOf,
  accuracyRadio,
  addButton,
  numberField,
  pickAccuracy,
  removeButton,
  section,
  unitSelect,
} from '../test/helpers';
import { must } from '../test/must';

// The Delayer section has one three-way toggle "Delay accuracy" (Basic / Reordering / Correlation) instead of Reordering and
// "Activate correlation" checkboxes. The DTO keeps both booleans. Reordering needs the token bucket limiter and excludes
// correlation. The tooltip text of the toggle is covered in tooltips.test.tsx. The toggle is queried by its radio buttons
// (react-bootstrap ToggleButtonGroup of type radio, or plain radios), each with the accessible name of its option.
const body = () => document.body;

const limiterMode = () => screen.getByLabelText('Limiter mode') as HTMLSelectElement;
const modeOption = (value: 'Spacer' | 'TokenBucketFilter') =>
  must(
    Array.from(limiterMode().options).find((o) => o.value === value),
    `option ${value}`,
  );
const REORDERING_NOTE = /Reordering works only with the policer/i;

/** A stored config with the given reordering flag, correlation flag and limiter mode (a delayer is always present). */
function stored(reordering: boolean, correlation: boolean, mode: 'Spacer' | 'TokenBucketFilter'): ConfigDto {
  return { ...cfg({ activate_correlation: correlation }, { mode }), reordering };
}

describe.each(PAGES)('$name: the "Delay accuracy" toggle (F1)', (page: ConfigPage) => {
  it('has exactly the three options Basic, Reordering, Correlation in the Delayer section, labelled "Delay accuracy"', async () => {
    await page.openFull();
    const delayer = section('Delayer');
    expect(
      within(delayer)
        .getAllByRole('radio')
        .map((r) => (r as HTMLInputElement).labels?.[0]?.textContent?.trim()),
    ).toEqual(ACCURACY_MODES);
    for (const m of ACCURACY_MODES) expect(delayer).toContainElement(accuracyRadio(m));
    expect(within(delayer).getAllByText('Delay accuracy').length).toBeGreaterThanOrEqual(1);
    expect(screen.getAllByRole('radio')).toHaveLength(3); // no other radios in the form
  });

  it('exactly one option is selected, Basic on a new form', async () => {
    await page.openFull();
    expect(accuracyOf()).toBe('Basic');
    expect(ACCURACY_MODES.filter((m) => accuracyRadio(m).checked)).toEqual(['Basic']);
  });

  it('the old checkboxes are gone: no "Reordering" in the Buffer section, no "Activate correlation" anywhere', async () => {
    await page.openFull();
    expect(screen.queryByRole('checkbox', { name: 'Reordering' })).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox', { name: 'Activate correlation' })).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Activate correlation')).not.toBeInTheDocument();
    expect(within(section('Buffer')).queryByLabelText('Reordering')).not.toBeInTheDocument();
    expect(within(section('Buffer')).queryAllByRole('radio')).toHaveLength(0);
  });

  it('is part of the Delayer stage: absent while the delayer is not added, back (with its choice) when re-added', async () => {
    const user = await page.openNew();
    expect(screen.queryByRole('radio', { name: 'Basic' })).not.toBeInTheDocument();
    await user.click(addButton('delayer'));
    await pickAccuracy(user, 'Correlation');
    await user.click(removeButton('delayer'));
    expect(screen.queryByRole('radio', { name: 'Correlation' })).not.toBeInTheDocument();
    await user.click(addButton('delayer'));
    expect(accuracyOf()).toBe('Correlation');
  });

  it('picking an option selects it and only it', async () => {
    const user = await page.openFull();
    for (const m of ['Reordering', 'Correlation', 'Basic', 'Correlation']) {
      await pickAccuracy(user, m);
      expect(accuracyOf()).toBe(m);
      expect(ACCURACY_MODES.filter((x) => accuracyRadio(x).checked)).toEqual([m]);
    }
  });

  it('the toggle is not a required field (no required marker, not required)', async () => {
    await page.openFull();
    for (const m of ACCURACY_MODES) expect(accuracyRadio(m)).not.toBeRequired();
  });

  it('the half-life field is enabled only with Correlation', async () => {
    const user = await page.openFull();
    const halfLife = () => numberField(body(), 'Half-life period');
    expect(halfLife()).toBeDisabled();
    await pickAccuracy(user, 'Correlation');
    expect(halfLife()).toBeEnabled();
    expect(unitSelect(body(), 'Half-life period')).toBeEnabled();
    await pickAccuracy(user, 'Reordering');
    expect(halfLife()).toBeDisabled();
    expect(unitSelect(body(), 'Half-life period')).toBeDisabled();
    await pickAccuracy(user, 'Basic');
    expect(halfLife()).toBeDisabled();
  });

  it.each([
    ['Basic', false, false],
    ['Reordering', true, false],
    ['Correlation', false, true],
  ] as [string, boolean, boolean][])(
    '%s is saved as reordering=%s, activate_correlation=%s',
    async (mode, reordering, corr) => {
      const user = await page.openFull();
      await page.ready(user);
      await pickAccuracy(user, mode);
      await page.save(user);
      const c = page.created();
      expect(c.reordering).toBe(reordering);
      expect(must(c.delayer).activate_correlation).toBe(corr);
    },
  );

  it('the DTO still has both booleans (no extra field, no API change)', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await pickAccuracy(user, 'Correlation');
    await page.save(user);
    const c = page.created();
    expect(typeof c.reordering).toBe('boolean');
    expect(Object.keys(must(c.delayer)).sort()).toEqual([
      'activate_correlation',
      'distribution',
      'half_life_period_ns',
      'initialization_rate_pps',
      'mean_ns',
      'std_ns',
    ]);
  });

  it('Correlation keeps its half-life requirement: an emptied half-life is refused, the alert names it', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await pickAccuracy(user, 'Correlation');
    await user.clear(numberField(body(), 'Half-life period'));
    await page.save(user);
    expect(await screen.findByRole('alert')).toHaveTextContent(/Half-life period/);
    expect(page.requests()).toBe(0);
  });

  it('Reordering does not require a half-life: an emptied half-life saves', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await pickAccuracy(user, 'Correlation');
    await user.clear(numberField(body(), 'Half-life period'));
    await pickAccuracy(user, 'Reordering');
    await page.save(user);
    expect(page.created().reordering).toBe(true);
  });

  it('without the Delayer stage the config is sent with Basic, even after Reordering was chosen', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await pickAccuracy(user, 'Reordering');
    await user.click(removeButton('delayer'));
    await page.save(user);
    const c = page.created();
    expect(c.delayer ?? null).toBeNull();
    expect(c.reordering).toBe(false);
  });

  it('a stored reordering=true without a delayer is sent back with Basic (reordering=false)', async () => {
    const user = await page.openStored({ ...cfg(null), reordering: true });
    await page.save(user);
    const c = page.updated();
    expect(c.delayer ?? null).toBeNull();
    expect(c.reordering).toBe(false);
  });
});

describe.each(PAGES)('$name: loading a stored config selects the option (F1)', (page: ConfigPage) => {
  it.each([
    [false, false, 'Basic'],
    [true, false, 'Reordering'],
    [false, true, 'Correlation'],
    [true, true, 'Reordering'], // reordering wins
  ] as [boolean, boolean, string][])(
    'reordering=%s, activate_correlation=%s is shown as %s',
    async (reordering, corr, shown) => {
      await page.openStored(stored(reordering, corr, 'TokenBucketFilter'));
      expect(accuracyOf()).toBe(shown);
    },
  );

  it('a stored Correlation has its half-life enabled', async () => {
    await page.openStored(stored(false, true, 'Spacer'));
    expect(numberField(body(), 'Half-life period')).toBeEnabled();
    expect(accuracyOf()).toBe('Correlation');
  });

  it('a stored Reordering config saves unchanged', async () => {
    const user = await page.openStored(stored(true, false, 'TokenBucketFilter'));
    await page.save(user);
    const c = page.updated();
    expect(c.reordering).toBe(true);
    expect(must(c.delayer).activate_correlation).toBe(false);
    expect(must(c.limiter).mode).toBe('TokenBucketFilter');
  });

  it('a stored Correlation config with the Spacer saves unchanged', async () => {
    const user = await page.openStored(stored(false, true, 'Spacer'));
    await page.save(user);
    const c = page.updated();
    expect(c.reordering).toBe(false);
    expect(must(c.delayer).activate_correlation).toBe(true);
    expect(must(c.limiter).mode).toBe('Spacer');
  });
});

describe.each(PAGES)('$name: Reordering and the limiter mode (F3)', (page: ConfigPage) => {
  it('with Basic or Correlation both modes are selectable and no note is shown', async () => {
    const user = await page.openFull();
    for (const m of ['Basic', 'Correlation']) {
      await pickAccuracy(user, m);
      expect(modeOption('Spacer')).toBeEnabled();
      expect(modeOption('TokenBucketFilter')).toBeEnabled();
      expect(limiterMode()).toBeEnabled();
      expect(within(section('Limiter')).queryByText(REORDERING_NOTE)).not.toBeInTheDocument();
    }
    expect(limiterMode()).toHaveValue('Spacer'); // the default mode is untouched
  });

  it('selecting Reordering while the mode is Spacer switches it to TokenBucketFilter, disables Spacer and says why', async () => {
    const user = await page.openFull();
    expect(limiterMode()).toHaveValue('Spacer');
    await pickAccuracy(user, 'Reordering');
    expect(limiterMode()).toHaveValue('TokenBucketFilter');
    expect(modeOption('Spacer')).toBeDisabled();
    expect(modeOption('TokenBucketFilter')).toBeEnabled();
    expect(within(section('Limiter')).getByText(REORDERING_NOTE)).toBeInTheDocument();
  });

  it('selecting Reordering while the mode is already TokenBucketFilter keeps it', async () => {
    const user = await page.openFull();
    await user.selectOptions(limiterMode(), 'TokenBucketFilter');
    await pickAccuracy(user, 'Reordering');
    expect(limiterMode()).toHaveValue('TokenBucketFilter');
    expect(modeOption('Spacer')).toBeDisabled();
  });

  it('leaving Reordering enables Spacer again and removes the note', async () => {
    const user = await page.openFull();
    await pickAccuracy(user, 'Reordering');
    await pickAccuracy(user, 'Correlation');
    expect(modeOption('Spacer')).toBeEnabled();
    expect(within(section('Limiter')).queryByText(REORDERING_NOTE)).not.toBeInTheDocument();
    await user.selectOptions(limiterMode(), 'Spacer');
    expect(limiterMode()).toHaveValue('Spacer');
    await pickAccuracy(user, 'Basic');
    expect(modeOption('Spacer')).toBeEnabled();
  });

  it('a limiter added after Reordering was chosen is the token bucket with Spacer disabled', async () => {
    const user = await page.openNew();
    await user.click(addButton('delayer'));
    await pickAccuracy(user, 'Reordering');
    await user.click(addButton('limiter'));
    expect(limiterMode()).toHaveValue('TokenBucketFilter');
    expect(modeOption('Spacer')).toBeDisabled();
    expect(within(section('Limiter')).getByText(REORDERING_NOTE)).toBeInTheDocument();
  });

  it('Reordering without a limiter stage is fine and sends reordering=true with no limiter', async () => {
    const user = await page.openNew();
    await user.click(addButton('delayer'));
    await page.ready(user);
    await pickAccuracy(user, 'Reordering');
    expect(screen.queryByLabelText('Limiter mode')).not.toBeInTheDocument();
    await page.save(user);
    const c = page.created();
    expect(c.reordering).toBe(true);
    expect(c.limiter ?? null).toBeNull();
  });

  it('Reordering from the Spacer default is saved with the token bucket and keeps the rate', async () => {
    const user = await page.openFull();
    await page.ready(user);
    await pickAccuracy(user, 'Reordering');
    await page.save(user);
    const c = page.created();
    expect(c.reordering).toBe(true);
    expect(c.limiter).toEqual({ mode: 'TokenBucketFilter', rate_bps: 1e10 });
  });

  it('a stored Reordering with the token bucket shows Spacer disabled and the note', async () => {
    await page.openStored(stored(true, false, 'TokenBucketFilter'));
    expect(limiterMode()).toHaveValue('TokenBucketFilter');
    expect(modeOption('Spacer')).toBeDisabled();
    expect(within(section('Limiter')).getByText(REORDERING_NOTE)).toBeInTheDocument();
  });

  it('a stored Basic or Correlation config with the Spacer shows it selectable and no note', async () => {
    await page.openStored(stored(false, true, 'Spacer'));
    expect(limiterMode()).toHaveValue('Spacer');
    expect(modeOption('Spacer')).toBeEnabled();
    expect(within(section('Limiter')).queryByText(REORDERING_NOTE)).not.toBeInTheDocument();
  });
});

describe.each(PAGES)('$name: invalid combinations are refused on save (F4)', (page: ConfigPage) => {
  it('a stored document with reordering and the Spacer is refused: alert naming the combination, nothing written', async () => {
    const user = await page.openStored(stored(true, false, 'Spacer'));
    await page.save(user);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(/reordering/i);
    expect(alert).toHaveTextContent(/spacer/i);
    expect(page.requests()).toBe(0);
  });

  it('a refused combination can be corrected (Correlation, or the token bucket) and saved', async () => {
    const user = await page.openStored(stored(true, false, 'Spacer'));
    await page.save(user);
    await screen.findByRole('alert');
    await pickAccuracy(user, 'Correlation');
    await user.selectOptions(limiterMode(), 'Spacer');
    await page.save(user);
    const c = page.updated();
    expect(c.reordering).toBe(false);
    expect(must(c.delayer).activate_correlation).toBe(true);
    expect(must(c.limiter).mode).toBe('Spacer');
  });
});
