import { screen, within } from '@testing-library/react';
import { PAGES, cfg } from '../test/configPages';
import type { ConfigPage } from '../test/configPages';
import { SECTIONS, numberField, section, typeInto, unitSelect } from '../test/helpers';
import * as fx from '../test/fixtures';

// Contract: "Subsections" and "Enable / disable" in the v16 block of src/test/helpers.tsx. Each test runs against the
// link editor and the preset editor (they share ConfigFieldset).
const body = () => document.body;
const outer = (page: ConfigPage) => screen.getByRole('group', { name: page.outerName });
const ctl = (label: string) => screen.getByLabelText(label) as HTMLInputElement;

// Labels (controls) that live in each subsection. The unit selects are "<label> unit".
const CONTENT: Record<string, string[]> = {
  Buffer: ['Buffer capacity', 'Reordering', 'Seed'],
  Delayer: [
    'Delay distribution', 'Mean delay', 'Mean delay unit', 'Delay std', 'Delay std unit', 'Activate correlation',
    'Half-life period', 'Half-life period unit', 'Initialization rate', 'Initialization rate unit',
  ],
  Limiter: ['Limiter active', 'Limiter mode', 'Rate', 'Rate unit'],
  Dropper: ['Loss model', 'Random loss probability'],
  Statistics: [
    'Delay statistics', 'Histogram bins', 'Histogram lo', 'Histogram lo unit', 'Histogram hi', 'Histogram hi unit',
  ],
};

describe.each(PAGES)('$name: subsections', (page: ConfigPage) => {
  it('keeps the outer fieldset and nests exactly five fieldsets, legends Buffer, Delayer, Limiter, Dropper, Statistics in order', async () => {
    await page.openNew();
    const nested = Array.from(outer(page).querySelectorAll('fieldset'));
    expect(nested.map((f) => f.querySelector(':scope > legend')?.textContent?.trim())).toEqual(SECTIONS);
    for (const f of nested) {
      expect(f.tagName).toBe('FIELDSET');
      expect(f.firstElementChild?.tagName).toBe('LEGEND');
      expect(within(outer(page)).getAllByRole('group', { name: f.querySelector('legend')?.textContent ?? '' })).toContain(f);
    }
    // the five are direct subsections, not nested into each other
    for (const f of nested) expect(f.parentElement?.closest('fieldset')).toBe(outer(page));
  });

  it('the subsections follow each other in the contracted order in the document', async () => {
    await page.openNew();
    for (let i = 0; i + 1 < SECTIONS.length; i += 1) {
      const a = section(SECTIONS[i]);
      const b = section(SECTIONS[i + 1]);
      expect(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
      expect(a.contains(b)).toBe(false);
    }
  });

  it.each(Object.entries(CONTENT))('%s holds its fields and no field of another subsection', async (name, labels) => {
    await page.openNew();
    const own = section(name);
    for (const l of labels) expect(within(own).getByLabelText(l)).toBeInTheDocument();
    for (const [other, otherLabels] of Object.entries(CONTENT)) {
      if (other === name) continue;
      for (const l of otherLabels) expect(within(own).queryByLabelText(l)).not.toBeInTheDocument();
    }
  });

  it('the Dropper holds the Gilbert-Elliot parameters once that model is chosen, and only them', async () => {
    const user = await page.openNew();
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    const d = section('Dropper');
    for (const l of ['Gilbert-Elliot p', 'Gilbert-Elliot r', 'Gilbert-Elliot k', 'Gilbert-Elliot h', 'Gilbert-Elliot starts in good state']) {
      expect(within(d).getByLabelText(l)).toBeInTheDocument();
    }
    expect(within(d).queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    expect(within(d).getByLabelText('Loss model')).toBeInTheDocument();
  });

  it('every control of the config form lies in a subsection, except the preset actions of the link editor', async () => {
    await page.openNew();
    const subs = SECTIONS.map(section);
    const loose = Array.from(outer(page).querySelectorAll<HTMLElement>('input, select, textarea, button')).filter(
      (c) => !subs.some((s) => s.contains(c)),
    );
    const names = loose.map((c) => (c.tagName === 'BUTTON' ? c.textContent : (c as HTMLInputElement).labels?.[0]?.textContent)?.trim());
    expect(names.sort()).toEqual(page.name === 'link editor' ? ['Apply preset', 'Save as preset'] : []);
  });

  it('the same structure shows when editing a stored config', async () => {
    await page.openStored(fx.config2());
    // nested fieldsets only: the outer one may be named by a legend or by aria-labelledby
    const nested = Array.from(outer(page).querySelectorAll('fieldset'));
    expect(nested.map((f) => f.querySelector(':scope > legend')?.textContent?.trim())).toEqual(SECTIONS);
    for (const l of CONTENT.Delayer) expect(within(section('Delayer')).getByLabelText(l)).toBeInTheDocument();
    for (const l of ['Gilbert-Elliot p', 'Gilbert-Elliot h']) expect(within(section('Dropper')).getByLabelText(l)).toBeInTheDocument();
  });
});

describe('link editor: what stays outside the subsections', () => {
  const link = PAGES[0];

  it('Apply preset and Save as preset belong to the whole configuration: inside the outer fieldset, in no subsection', async () => {
    await link.openNew();
    const apply = ctl('Apply preset');
    const saveAs = screen.getByRole('button', { name: 'Save as preset' });
    expect(outer(link)).toContainElement(apply);
    expect(outer(link)).toContainElement(saveAs);
    for (const s of SECTIONS.map(section)) {
      expect(s).not.toContainElement(apply);
      expect(s).not.toContainElement(saveAs);
    }
  });

  it('Name, Worker lcore, Ingress and Egress are outside the config fieldset', async () => {
    await link.openNew();
    for (const l of ['Name', 'Worker lcore', 'Ingress', 'Egress']) {
      expect(outer(link)).not.toContainElement(ctl(l));
    }
  });
});

describe('preset editor: what stays outside the subsections', () => {
  const preset = PAGES[1];

  it('has no Apply preset and no Save as preset (they are link-editor features)', async () => {
    await preset.openNew();
    expect(screen.queryByLabelText('Apply preset')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Save as preset' })).not.toBeInTheDocument();
  });

  it('Name and Description are outside the config fieldset', async () => {
    await preset.openNew();
    for (const l of ['Name', 'Description']) expect(outer(preset)).not.toContainElement(ctl(l));
  });
});

describe.each(PAGES)('$name: switch dependent fields are disabled, not hidden, and keep their values', (page: ConfigPage) => {
  const halfLife = () => ctl('Half-life period');
  const limiterFields = ['Limiter mode', 'Rate', 'Rate unit'];

  it('half-life and its unit are disabled while "Activate correlation" is off and enabled while it is on', async () => {
    const user = await page.openNew();
    expect(ctl('Activate correlation')).not.toBeChecked();
    expect(halfLife()).toBeInTheDocument();
    expect(halfLife()).toBeDisabled();
    expect(unitSelect(body(), 'Half-life period')).toBeDisabled();
    await user.click(ctl('Activate correlation'));
    expect(halfLife()).toBeEnabled();
    expect(unitSelect(body(), 'Half-life period')).toBeEnabled();
    await user.click(ctl('Activate correlation'));
    expect(halfLife()).toBeDisabled();
    expect(unitSelect(body(), 'Half-life period')).toBeDisabled();
  });

  it('the initialization rate and its unit are enabled whatever the correlation switch says', async () => {
    const user = await page.openNew();
    expect(ctl('Initialization rate')).toBeEnabled();
    expect(unitSelect(body(), 'Initialization rate')).toBeEnabled();
    await user.click(ctl('Activate correlation'));
    expect(ctl('Initialization rate')).toBeEnabled();
    expect(unitSelect(body(), 'Initialization rate')).toBeEnabled();
  });

  it('mode, rate, limit and their units are disabled while "Limiter active" is off and enabled while it is on', async () => {
    const user = await page.openNew();
    expect(ctl('Limiter active')).not.toBeChecked();
    for (const l of limiterFields) expect(ctl(l)).toBeDisabled();
    await user.click(ctl('Limiter active'));
    for (const l of limiterFields) expect(ctl(l)).toBeEnabled();
    await user.click(ctl('Limiter active'));
    for (const l of limiterFields) expect(ctl(l)).toBeDisabled();
    expect(ctl('Limiter active')).toBeEnabled(); // the switch itself always works
  });

  it('a typed half-life survives switching correlation off and on and is still sent while it is off', async () => {
    const user = await page.openNew();
    await user.click(ctl('Activate correlation'));
    await typeInto(user, body(), 'Half-life period', '5', 'ms');
    await user.click(ctl('Activate correlation'));
    expect(halfLife()).toHaveValue(5);
    await user.click(ctl('Activate correlation'));
    expect(halfLife()).toHaveValue(5);
    await user.click(ctl('Activate correlation'));
    await page.ready(user);
    await page.save(user);
    const d = page.created().delayer;
    expect(d.activate_correlation).toBe(false);
    expect(d.half_life_period_ns).toBe(5000000);
  });

  it('typed limiter values survive switching the limiter off and on and are still sent while it is off', async () => {
    const user = await page.openNew();
    await user.click(ctl('Limiter active'));
    await user.selectOptions(ctl('Limiter mode'), 'TokenBucketFilter');
    await typeInto(user, body(), 'Rate', '2.5', 'Gbit/s');
    await user.click(ctl('Limiter active'));
    await user.click(ctl('Limiter active'));
    expect(ctl('Rate')).toHaveValue(2.5);
    expect(ctl('Limiter mode')).toHaveValue('TokenBucketFilter');
    await user.click(ctl('Limiter active'));
    await page.ready(user);
    await page.save(user);
    expect(page.created().limiter).toEqual({ mode: 'TokenBucketFilter', activate: false, rate_bps: 2500000000 });
  });

  it('with the switches on, a stored config has every dependent field enabled', async () => {
    await page.openStored(fx.config2());
    expect(halfLife()).toBeEnabled();
    for (const l of limiterFields) expect(ctl(l)).toBeEnabled();
  });

  it('with the switches off, a stored config shows its values in disabled fields', async () => {
    await page.openStored(cfg({ activate_correlation: false, half_life_period_ns: 4000000 }, { activate: false, rate_bps: 5000000000 }));
    expect(halfLife()).toBeDisabled();
    expect(halfLife()).toHaveValue(4);
    expect(ctl('Rate')).toBeDisabled();
    expect(ctl('Rate')).toHaveValue(5);
  });

  it('everything else stays enabled whatever the switches say', async () => {
    await page.openNew();
    for (const l of ['Delay distribution', 'Mean delay', 'Mean delay unit', 'Delay std', 'Delay std unit', 'Activate correlation',
      'Limiter active', 'Loss model', 'Random loss probability', 'Buffer capacity', 'Reordering', 'Seed', 'Delay statistics',
      'Histogram bins', 'Histogram lo', 'Histogram lo unit', 'Histogram hi', 'Histogram hi unit']) {
      expect(ctl(l)).toBeEnabled();
    }
  });

  it('the histogram fields are not switch dependent: enabled while "Delay statistics" is off', async () => {
    const user = await page.openNew();
    await user.click(ctl('Delay statistics'));
    expect(ctl('Delay statistics')).not.toBeChecked();
    for (const l of ['Histogram bins', 'Histogram lo', 'Histogram lo unit', 'Histogram hi', 'Histogram hi unit']) {
      expect(ctl(l)).toBeEnabled();
    }
  });

  it('typed loss parameters survive switching the loss model away and back', async () => {
    const user = await page.openNew();
    await user.clear(ctl('Random loss probability'));
    await user.type(ctl('Random loss probability'), '0.05');
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    await user.clear(ctl('Gilbert-Elliot p'));
    await user.type(ctl('Gilbert-Elliot p'), '0.02');
    await user.selectOptions(ctl('Loss model'), 'Random');
    expect(ctl('Random loss probability')).toHaveValue(0.05);
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    expect(ctl('Gilbert-Elliot p')).toHaveValue(0.02);
  });

  it('only the loss model that is selected is rendered (the alternatives are hidden, not disabled)', async () => {
    const user = await page.openNew();
    expect(screen.queryByLabelText('Gilbert-Elliot p')).not.toBeInTheDocument();
    await user.selectOptions(ctl('Loss model'), 'GilbertElliot');
    expect(screen.queryByLabelText('Random loss probability')).not.toBeInTheDocument();
    expect(numberField(body(), 'Gilbert-Elliot r')).toBeEnabled();
  });
});
